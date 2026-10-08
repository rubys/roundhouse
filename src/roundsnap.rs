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
//! After [`crate::project::write_to_dir`], the CRuby emit path calls
//! [`crate::project::finalize_roundsnap`] / [`finalize`] so the host
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
        // test_helper.rb require_relatives these after boot; they are not
        // on the app boot closure, so without this they vanish and the
        // Campfire suite cannot load. Keep as .rb (not ISeq) — small.
        || path == "runtime/resolv.rb"
        || path == "runtime/tcp_socket_stub.rb"
        || path == "runtime/secure_random_stub.rb"
        || !path.ends_with(".rb")
}

/// Rewrite the CRuby file set for Roundsnap ISeq delivery. Idempotent if
/// `units.json` is already present.
pub fn prepare(_app: &App, files: &mut Vec<(String, String)>) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    if files.iter().any(|(p, _)| p == "units.json") {
        return Ok(());
    }

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
    let mut ordered_keys = transitive_rb_closure(&seeds, &by_path);
    // Controllers (and other app/*) are often required at request time
    // via Main.instantiate_controller, not from boot.rb. Without them in
    // the manifest, Puma boots but every route LoadErrors on missing .rb.
    for path in by_path.keys() {
        if path.starts_with("app/") && path.ends_with(".rb") && !ordered_keys.iter().any(|k| k == path)
        {
            ordered_keys.push(path.clone());
        }
    }
    // Never compile classic boot.rb as a unit: on-disk boot.rb becomes
    // THIN_BOOT (Loader.boot!). Including the classic chain as unit
    // `boot` made main.rb's `require_relative "boot"` re-enter the full
    // boot ISeq beside the thin file (Thermos dual-boot finding).
    let ordered_keys: Vec<String> = ordered_keys
        .into_iter()
        .filter(|k| k != "boot.rb" && k != "boot")
        .collect();

    let mut units = Vec::new();
    for key in &ordered_keys {
        let source = by_path
            .get(key)
            .ok_or_else(|| format!("roundsnap::prepare: missing source for {key}"))?
            .clone();
        // Drop require_relative "boot" — thin boot already owns startup.
        let source = strip_boot_require(&source);
        let stem = key_for_manifest(key);
        // Marked sources: leave `file` unset so the gem's SourceMap
        // expands contiguous spans with original paths + padded lines.
        // Unmarked: emit path (honest) — never original path + emitted
        // linenos (Sam: a wrong file:line is worse than an inconvenient one).
        if source.contains("#<SPINEL_SOURCE>") {
            // Gem SourceMap expands spans → original file + padded lines.
            units.push(serde_json::json!({
                "key": stem,
                "source": source,
            }));
        } else {
            units.push(serde_json::json!({
                "key": stem,
                "source": source,
                "file": key,
                "first_lineno": 1,
            }));
        }
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
    // Prefer compiling under YJIT when the host can — campfire's ruby /
    // roundsnap lanes set RUBY_YJIT_ENABLE=1, and a matching description
    // avoids an unnecessary rebuild. The loader also normalizes +YJIT.
    let output = Command::new(&exe)
        .arg("--out")
        .arg(dest)
        .arg("--units")
        .arg(&units)
        .env("RUBY_YJIT_ENABLE", "1")
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
    // Same root as `vendor_gem` — never a CWD-relative checkout that
    // could disagree with the vendored loader.
    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gems/roundsnap/exe/roundsnap-compile");
    if exe.is_file() {
        return Ok(exe);
    }
    Err(format!(
        "roundsnap::finalize: {} not found",
        exe.display()
    ))
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
        "lib/roundsnap/source_map.rb",
        "exe/roundsnap-compile",
    ] {
        let src = root.join(rel);
        let text = std::fs::read_to_string(&src)
            .map_err(|e| format!("roundsnap::prepare: read {}: {e}", src.display()))?;
        by_path.insert(format!("vendor/roundsnap/{rel}"), text);
    }
    Ok(())
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
/// Remove `require_relative "boot"` lines so compiled units do not pull
/// a classic boot ISeq (or miss a deleted unit) after thin boot owns startup.
fn strip_boot_require(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let t = line.trim();
        let is_boot = matches!(
            t,
            "require_relative \"boot\""
                | "require_relative 'boot'"
                | "require_relative \"boot.rb\""
                | "require_relative 'boot.rb'"
        );
        if is_boot {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

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

fn patch_config_ru(ru: &str) -> String {
    // Puma loads config.ru, not thin boot.rb. Must boot! the ISeq units
    // before require_relative main/cable — main's require_relative "boot"
    // was stripped, so install-only leaves ActiveRecord undefined.
    if ru.contains("Roundsnap::Loader") && ru.contains(".boot!") {
        return ru.to_string();
    }
    let inject = "require \"roundsnap\"\n\
Roundsnap::Loader.install!(root: __dir__).boot!\n\n";
    // Replace a prior install-only inject from an older emit.
    let ru = if ru.contains("Roundsnap::Loader.install!(root: __dir__)\n")
        && !ru.contains(".boot!")
    {
        ru.replacen(
            "Roundsnap::Loader.install!(root: __dir__)\n",
            "Roundsnap::Loader.install!(root: __dir__).boot!\n",
            1,
        )
    } else {
        ru.to_string()
    };
    if ru.contains("Roundsnap::Loader") {
        return ru;
    }
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
    fn strip_boot_require_drops_boot_lines() {
        let src = "require_relative \"runtime/x\"\nrequire_relative \"boot\"\nX = 1\n";
        assert_eq!(
            strip_boot_require(src),
            "require_relative \"runtime/x\"\nX = 1\n"
        );
    }

    #[test]
    fn enabled_reads_roundsnap_or_legacy_alias() {
        // Cargo runs tests on parallel threads; serialize env mutation.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
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
