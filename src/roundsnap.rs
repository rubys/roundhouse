//! CRuby straight-to-ISeq delivery via the in-repo **roundsnap** gem.
//!
//! When `ROUNDSNAP=1` (alias: `ROUNDHOUSE_RUBY_ISEQ=1`), the in-memory
//! file set is rewritten to:
//!
//! - `units.json` — lowered sources + original `file` metadata
//! - `vendor/roundsnap/` — vendored copy of `gems/roundsnap`
//! - thin `boot.rb` / patched `config.ru` / Gemfile path gem
//! - app/runtime `.rb` bodies removed (unless `ROUNDSNAP_KEEP_SOURCE=1`)
//!
//! After [`crate::project::write_to_dir`], call [`finalize`] so the host
//! Ruby runs `roundsnap-compile` and writes binary `iseq/**`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::App;

/// Opt-in. Set `ROUNDSNAP=1` (or legacy `ROUNDHOUSE_RUBY_ISEQ=1`) to emit
/// the ISeq artifact for `--target ruby`. Unset / `0` keeps the classic
/// `.rb` tree (default), so existing toolchain tests stay on the known layout.
pub fn enabled() -> bool {
    env_flag("ROUNDSNAP") || env_flag("ROUNDHOUSE_RUBY_ISEQ")
}

fn keep_source() -> bool {
    env_flag("ROUNDSNAP_KEEP_SOURCE") || env_flag("ROUNDHOUSE_ISEQ_KEEP_SOURCE")
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("yes") | Ok("on")
    )
}

/// Paths that must remain as source on disk (bundler, puma, rack, tests).
fn keep_as_source(path: &str) -> bool {
    path == "config.ru"
        || path == "Rakefile"
        || path == "Gemfile"
        || path == "Gemfile.lock"
        || path == "Makefile"
        || path == "boot.rb"
        || path == "units.json"
        || path.starts_with("config/")
        || path.starts_with("vendor/")
        || path.starts_with("test/")
        || path.starts_with("e2e/")
        || !path.ends_with(".rb")
}

/// Rewrite the CRuby file set for Roundsnap ISeq delivery. Idempotent if
/// `units.json` is already present.
pub fn prepare(app: &App, files: &mut Vec<(String, String)>) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    if files.iter().any(|(p, _)| p == "units.json") {
        return Ok(());
    }

    let label = app_label(app);
    let mut by_path: BTreeMap<String, String> = files.iter().cloned().collect();

    let boot = by_path
        .get("boot.rb")
        .ok_or("roundsnap::prepare: boot.rb missing from CRuby file set")?
        .clone();
    // Boot-chain first, then the Rack entry points. Do NOT dump every
    // leftover .rb into the manifest — spinel-only files (e.g.
    // runtime/cable.rb needing Tep) still sit in the file set until
    // stripped below and would break boot! if compiled.
    let mut seeds = boot_require_keys(&boot);
    for extra in ["main.rb", "cable.rb", "runtime/gzip_cache.rb"] {
        if by_path.contains_key(extra) && !seeds.iter().any(|k| k == extra) {
            seeds.push(extra.to_string());
        }
    }
    let ordered_keys = transitive_rb_closure(&seeds, &by_path);

    let mut units = Vec::new();
    for key in &ordered_keys {
        let source = by_path
            .get(key)
            .ok_or_else(|| format!("roundsnap::prepare: missing source for {key}"))?
            .clone();
        let (file, first_lineno) = original_location(app, &label, key);
        units.push(serde_json::json!({
            "key": key_for_manifest(key),
            "source": source,
            "file": file,
            "first_lineno": first_lineno,
        }));
    }

    let units_json = serde_json::to_string_pretty(&units)
        .map_err(|e| format!("roundsnap::prepare: serialize units: {e}"))?;
    by_path.insert("units.json".to_string(), units_json + "\n");

    vendor_gem(&mut by_path)?;

    let gemfile = by_path
        .get_mut("Gemfile")
        .ok_or("roundsnap::prepare: Gemfile missing")?;
    if !gemfile.contains("roundsnap") {
        gemfile.push_str(
            "\n# MRI ISeq delivery — vendored from gems/roundsnap\n\
             gem \"roundsnap\", path: \"vendor/roundsnap\"\n",
        );
    }
    // Scaffold lock is MRI-without-roundsnap; drop it so `bundle
    // install` in the emitted tree resolves the path gem.
    by_path.remove("Gemfile.lock");

    by_path.insert("boot.rb".to_string(), THIN_BOOT.to_string());
    if let Some(ru) = by_path.get_mut("config.ru") {
        *ru = patch_config_ru(ru);
    }

    if !keep_source() {
        // Only non-.rb assets + keep_as_source stubs remain; compiled
        // bodies live in iseq/. Also drops spinel-only leftovers
        // (runtime/cable.rb) that must not sit beside overlay cable.rb.
        by_path.retain(|p, _| keep_as_source(p) || !p.ends_with(".rb"));
    }

    files.clear();
    files.extend(by_path);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(())
}

/// Run the gem compiler in `dest` after the text tree was written.
/// No-op when Roundsnap mode is off or `units.json` is absent.
/// Always recompiles when `units.json` is present (stale `manifest.json`
/// / `iseq/**` from a prior emit are replaced).
pub fn finalize(dest: &Path) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    let units = dest.join("units.json");
    if !units.is_file() {
        return Ok(());
    }

    // Drop prior artifacts so a re-emit cannot leave stale binaries.
    let _ = std::fs::remove_file(dest.join("manifest.json"));
    let _ = std::fs::remove_dir_all(dest.join("iseq"));

    let exe = compiler_exe()?;
    let output = Command::new(&exe)
        .arg("--out")
        .arg(dest)
        .arg("--units")
        .arg(&units)
        .output()
        .map_err(|e| format!("roundsnap::finalize: spawn {}: {e}", exe.display()))?;
    if !output.status.success() {
        return Err(format!(
            "roundsnap::finalize: compiler failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ));
    }

    // units.json is a compile input, not a runtime need.
    let _ = std::fs::remove_file(units);
    Ok(())
}

fn compiler_exe() -> Result<PathBuf, String> {
    let candidates = [
        PathBuf::from("gems/roundsnap/exe/roundsnap-compile"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gems/roundsnap/exe/roundsnap-compile"),
    ];
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    Err(
        "roundsnap::finalize: gems/roundsnap/exe/roundsnap-compile not found \
         (run from the roundhouse checkout)"
            .to_string(),
    )
}

fn vendor_gem(by_path: &mut BTreeMap<String, String>) -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gems/roundsnap");
    if !root.is_dir() {
        return Err(format!(
            "roundsnap::prepare: gem source missing at {}",
            root.display()
        ));
    }
    for rel in [
        "roundsnap.gemspec",
        "README.md",
        "lib/roundsnap.rb",
        "lib/roundsnap/version.rb",
        "lib/roundsnap/compiler.rb",
        "lib/roundsnap/loader.rb",
        "exe/roundsnap-compile",
    ] {
        let src = root.join(rel);
        let text = std::fs::read_to_string(&src)
            .map_err(|e| format!("roundsnap::prepare: read {}: {e}", src.display()))?;
        by_path.insert(format!("vendor/roundsnap/{rel}"), text);
    }
    Ok(())
}

fn app_label(app: &App) -> String {
    Path::new(app.root.trim_end_matches('/'))
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_string()
}

fn key_for_manifest(emit_path: &str) -> String {
    emit_path
        .strip_suffix(".rb")
        .unwrap_or(emit_path)
        .to_string()
}

/// Expand `require_relative` edges inside unit sources until closure.
fn transitive_rb_closure(
    seeds: &[String],
    by_path: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut ordered = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = seeds.to_vec();
    while let Some(path) = stack.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        if !by_path.contains_key(&path) {
            continue;
        }
        ordered.push(path.clone());
        if let Some(src) = by_path.get(&path) {
            for dep in require_relative_targets(src, &path) {
                if by_path.contains_key(&dep) && !seen.contains(&dep) {
                    stack.push(dep);
                }
            }
        }
    }
    // Prefer seed order for the boot prefix, then the rest stably.
    let mut final_order = Vec::new();
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for s in seeds {
        if ordered.iter().any(|k| k == s) && placed.insert(s.clone()) {
            final_order.push(s.clone());
        }
    }
    let mut rest: Vec<String> = ordered
        .into_iter()
        .filter(|k| !placed.contains(k))
        .collect();
    rest.sort();
    final_order.extend(rest);
    final_order
}

fn require_relative_targets(source: &str, from_path: &str) -> Vec<String> {
    let base = Path::new(from_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(""));
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("require_relative ") {
            let q = rest.trim().trim_matches('"').trim_matches('\'');
            if q.is_empty() {
                continue;
            }
            let joined = base.join(q);
            let mut s = normalize_rel_path(&joined.to_string_lossy().replace('\\', "/"));
            if !s.ends_with(".rb") {
                s.push_str(".rb");
            }
            out.push(s);
        }
    }
    out
}

fn normalize_rel_path(path: &str) -> String {
    let mut parts = Vec::new();
    for p in path.split('/') {
        if p.is_empty() || p == "." {
            continue;
        }
        if p == ".." {
            parts.pop();
        } else {
            parts.push(p);
        }
    }
    parts.join("/")
}

/// Emit-path keys (`runtime/foo.rb`) in boot.rb `require_relative` order.
fn boot_require_keys(boot: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in boot.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("require_relative ") {
            let q = rest.trim().trim_matches('"').trim_matches('\'');
            if q.is_empty() {
                continue;
            }
            let path = if q.ends_with(".rb") {
                q.to_string()
            } else {
                format!("{q}.rb")
            };
            keys.push(path);
        }
    }
    keys
}

/// Map an emitted path to the original app path for ISeq `file` metadata.
fn original_location(app: &App, label: &str, emit_path: &str) -> (String, i64) {
    let root = app.root.trim_end_matches('/');
    let emit_no_rb = emit_path.strip_suffix(".rb").unwrap_or(emit_path);

    for src in &app.sources {
        let rel = src
            .path
            .strip_prefix(root)
            .map(|r| r.trim_start_matches('/'))
            .unwrap_or(src.path.as_str());
        let rel_no_rb = rel.strip_suffix(".rb").unwrap_or(rel);

        if rel == emit_path || rel_no_rb == emit_no_rb {
            return (format!("{label}/{rel}"), 1);
        }

        // ERB → app/views/<name>.rb
        if let Some(without_erb) = rel.strip_suffix(".erb") {
            let stem = without_erb
                .strip_suffix(".html")
                .or_else(|| without_erb.strip_suffix(".json"))
                .or_else(|| without_erb.strip_suffix(".js"))
                .or_else(|| without_erb.strip_suffix(".text"))
                .unwrap_or(without_erb);
            if stem == emit_no_rb || format!("{stem}") == emit_no_rb {
                return (format!("{label}/{rel}"), 1);
            }
            // json view emit uses _json suffix
            if emit_no_rb == format!("{stem}_json") || emit_path.ends_with("_json.rb") {
                if stem.replace('\\', "/") == emit_no_rb.trim_end_matches("_json") {
                    return (format!("{label}/{rel}"), 1);
                }
            }
        }
    }

    (emit_path.to_string(), 1)
}

fn patch_config_ru(ru: &str) -> String {
    // Ensure the loader is installed before require_relative main/cable.
    // Avoid double-boot: thin boot.rb already ran boot!; config.ru only
    // installs the require hook so subsequent requires resolve via iseq.
    if ru.contains("roundsnap") || ru.contains("Roundsnap::Loader") {
        return ru.to_string();
    }
    let inject = "require \"roundsnap\"\n\
Roundsnap::Loader.install!(root: __dir__)\n\n";
    if let Some(i) = ru.find("require \"rack\"\n") {
        let mut out = String::new();
        out.push_str(&ru[..i]);
        out.push_str("require \"rack\"\n");
        out.push_str(inject);
        out.push_str(&ru[i + "require \"rack\"\n".len()..]);
        out
    } else {
        format!("{inject}{ru}")
    }
}

const THIN_BOOT: &str = "\
# frozen_string_literal: true
# Roundsnap ISeq delivery: load every compiled unit via the roundsnap gem.
# Set ROUNDSNAP=0 at emit time for the classic require_relative boot.
require \"roundsnap\"
Roundsnap::Loader.install!(root: __dir__).boot!
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_require_keys_extracts_relatives() {
        let boot = r#"
require "time"
require_relative "runtime/sqlite_adapter"
require_relative "app/models"
# comment
require_relative 'app/views'
"#;
        assert_eq!(
            boot_require_keys(boot),
            vec![
                "runtime/sqlite_adapter.rb".to_string(),
                "app/models.rb".to_string(),
                "app/views.rb".to_string(),
            ]
        );
    }

    #[test]
    fn key_for_manifest_strips_rb() {
        assert_eq!(key_for_manifest("app/models/article.rb"), "app/models/article");
        assert_eq!(key_for_manifest("main"), "main");
    }

    #[test]
    fn enabled_reads_roundsnap_or_legacy_alias() {
        unsafe {
            std::env::remove_var("ROUNDSNAP");
            std::env::remove_var("ROUNDHOUSE_RUBY_ISEQ");
        }
        assert!(!enabled());
        unsafe {
            std::env::set_var("ROUNDSNAP", "1");
        }
        assert!(enabled());
        unsafe {
            std::env::remove_var("ROUNDSNAP");
            std::env::set_var("ROUNDHOUSE_RUBY_ISEQ", "1");
        }
        assert!(enabled());
        unsafe {
            std::env::remove_var("ROUNDHOUSE_RUBY_ISEQ");
        }
    }
}
