//! Embedded Ruby declarations for constant resolution on every target.
//!
//! Host emitters also use the full Ruby and Spinel runtime table.
//! The wasm build embeds only Ruby implementations that declare
//! framework or Spinel shim constants.

/// `(path relative to the repository root, content)`, sorted by path.
#[cfg(not(target_arch = "wasm32"))]
static FILES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/runtime_files.rs"));
#[cfg(target_arch = "wasm32")]
static FILES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/runtime_const_sources.rs"));

/// Directories no walk descends into — the same list the app-directory
/// walkers in `project.rs` apply, so a scaffold's `static/` or the
/// `ruby_overlay/` (walked separately) stay out of the base set.
const SKIP_DIRS: &[&str] = &[
    "vendor", "node_modules", "build", "static", "tmp", "coverage", "log", ".bundle",
    "ruby_overlay",
];

/// The embedded file at `path` (`runtime/spinel/db.rb`), or an error
/// naming it — the same shape `fs::read_to_string` gave the callers.
pub fn read_to_string(path: &str) -> Result<String, String> {
    read(path)
        .map(str::to_string)
        .ok_or_else(|| format!("no embedded runtime file {path}"))
}

/// The embedded file at `path`, if any.
pub fn read(path: &str) -> Option<&'static str> {
    FILES
        .binary_search_by(|(p, _)| p.cmp(&path))
        .ok()
        .map(|i| FILES[i].1)
}

pub fn exists(path: &str) -> bool {
    read(path).is_some()
}

/// Every embedded file under `dir`, as `(path relative to dir, content)`,
/// skipping anything inside a `SKIP_DIRS` directory below `dir`.
fn under(dir: &str) -> impl Iterator<Item = (&'static str, &'static str)> {
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    FILES.iter().filter_map(move |(p, c)| {
        let rel = p.strip_prefix(prefix.as_str())?;
        let dirs = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        if dirs.split('/').any(|d| SKIP_DIRS.contains(&d)) {
            return None;
        }
        Some((rel, *c))
    })
}

/// Real framework and shim declarations, without tests or emit scaffolds.
pub(crate) fn ruby_sources() -> impl Iterator<Item = (&'static str, &'static str)> {
    FILES
        .iter()
        .filter(|(path, _)| {
            (path.starts_with("runtime/ruby/") || path.starts_with("runtime/spinel/"))
                && path.ends_with(".rb")
                && !path
                    .split('/')
                    .any(|part| part == "test" || part == "scaffold")
        })
        .map(|(path, text)| (*path, *text))
}

fn require_dir(dir: &str) -> Result<(), String> {
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    if FILES.iter().any(|(p, _)| p.starts_with(&prefix)) {
        Ok(())
    } else {
        Err(format!("missing {dir}/"))
    }
}

/// Recursive walk: every file under `dir` as `(prefix + relative path,
/// content)`.
pub fn walk_into(dir: &str, prefix: &str, out: &mut Vec<(String, String)>) -> Result<(), String> {
    require_dir(dir)?;
    for (rel, content) in under(dir) {
        out.push((format!("{prefix}{rel}"), content.to_string()));
    }
    Ok(())
}

/// Recursive walk routing `.rb` files under `rb_prefix` and `.rbs`
/// files under `rbs_prefix`; other extensions skipped.
pub fn walk_partitioned(
    dir: &str,
    rb_prefix: &str,
    rbs_prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    require_dir(dir)?;
    for (rel, content) in under(dir) {
        let prefix = match rel.rsplit_once('.').map(|(_, e)| e) {
            Some("rb") => rb_prefix,
            Some("rbs") => rbs_prefix,
            _ => continue,
        };
        out.push((format!("{prefix}{rel}"), content.to_string()));
    }
    Ok(())
}

/// Non-recursive walk: the files directly under `dir` whose extension
/// is in `exts`, as `(prefix + file name, content)`.
pub fn walk_flat(
    dir: &str,
    exts: &[&str],
    prefix: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    require_dir(dir)?;
    for (rel, content) in under(dir) {
        if rel.contains('/') {
            continue;
        }
        let Some((_, ext)) = rel.rsplit_once('.') else { continue };
        if !exts.contains(&ext) {
            continue;
        }
        out.push((format!("{prefix}{rel}"), content.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_answers_lookups() {
        assert!(FILES.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(exists("runtime/spinel/scaffold/Makefile"));
        assert!(read("runtime/ruby/rails.rb").is_some());
        assert!(!exists("runtime/ruby/.hidden"));
    }

    /// Three stores answer `Rails.cache` — the shared runtime's, the
    /// CRuby overlay's MemoryStore, and Spinel's sharded twin — and
    /// every method the compiler emits a call to has to exist on all of
    /// them. `read_str`/`write_str` once landed on one and 500'd every
    /// Campfire page on CRuby; `increment_str` landed on two and broke
    /// sign-in on the CRuby compare lane the same way.
    #[test]
    fn every_typed_cache_seam_method_exists_on_all_three_stores() {
        const SEAM: &[&str] = &["fetch_str", "read_str", "write_str", "increment_str"];
        // The overlay is its own class: the whole seam, by itself.
        for path in ["runtime/ruby/rails.rb", "runtime/spinel/scaffold/ruby_overlay/runtime/rails_cache.rb"] {
            let src = read(path).unwrap_or_else(|| panic!("{path} is embedded"));
            for m in SEAM {
                assert!(
                    src.contains(&format!("def {m}(")),
                    "{path} does not define `{m}` — every store behind Rails.cache must"
                );
            }
        }
        // The Spinel twin is a REOPEN of the shared class: it inherits
        // `fetch_str`, but every method that reads or writes the shared
        // `@entries`/`@expires_at` directly must be redefined over the
        // shards, or it silently uses the empty unsharded hashes.
        let twin = read("runtime/spinel/fragment_cache.rb").unwrap();
        for m in ["read_str", "write_str", "increment_str", "forget"] {
            assert!(
                twin.contains(&format!("def {m}(")),
                "runtime/spinel/fragment_cache.rb does not shard `{m}`"
            );
        }
        // CRuby MemoryStore and Spinel's Cache both fan out 32 ways with
        // the same `shard_of` mix — drift here puts room-page keys on
        // different contention shapes per lane for no reason. Pin the
        // formula body, not just the method name.
        let overlay =
            read("runtime/spinel/scaffold/ruby_overlay/runtime/rails_cache.rb").unwrap();
        assert!(
            overlay.contains("SHARD_COUNT = 32") && twin.contains("SHARD_COUNT = 32"),
            "CRuby overlay and Spinel fragment cache must both use 32 shards"
        );
        const SHARD_MIX: &str =
            "((k.getbyte(n - 1) * 31) + k.getbyte(n / 2)) % SHARD_COUNT";
        assert!(
            overlay.contains(SHARD_MIX) && twin.contains(SHARD_MIX),
            "overlay and Spinel shard_of must share the tail-byte mix"
        );
    }

    #[test]
    fn walks_mirror_the_disk_rules() {
        let mut base = Vec::new();
        walk_into("runtime/spinel/scaffold", "", &mut base).unwrap();
        assert!(base.iter().any(|(p, _)| p == "Makefile"));
        assert!(
            base.iter().all(|(p, _)| !p.starts_with("ruby_overlay/")),
            "SKIP_DIRS applies below the walked root"
        );
        let mut flat = Vec::new();
        walk_flat("runtime/spinel", &["rb"], "runtime/", &mut flat).unwrap();
        assert!(flat.iter().all(|(p, _)| p.starts_with("runtime/") && p.ends_with(".rb")));
        assert!(flat.iter().all(|(p, _)| p["runtime/".len()..].find('/').is_none()));
        assert!(walk_into("runtime/nope", "", &mut Vec::new()).is_err());
    }
}
