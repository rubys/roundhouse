//! Every bundled gem the emitted tree requires has to be in its
//! Gemfile.
//!
//! Since Ruby 3.4 a bundled gem ships with the interpreter but is NOT
//! on the load path under bundler unless a Gemfile names it. `boot.rb`
//! requires `bigdecimal`; the runtime requires `base64` and `resolv`.
//! An emitted tree that does not name them fails to load before it
//! reaches a single line of app code:
//!
//! ```text
//! ! Unable to load application: LoadError: cannot load such file -- bigdecimal
//! ```
//!
//! The blog fixture hid it: its asset group pulls `turbo-rails`, and
//! through it `activesupport`, which depends on all three. An app that
//! ships no asset pipeline gets no such group, and no such gems.
//!
//! Structural rather than a literal list, so it stays true as the
//! runtime grows a require.

use std::collections::BTreeSet;
use std::path::Path;

/// Gems that ship with Ruby but are not default gems as of 3.4 — the
/// ones bundler hides unless the Gemfile names them.
const BUNDLED_GEMS: &[&str] = &[
    "abbrev", "base64", "bigdecimal", "csv", "drb", "getoptlong", "mutex_m", "nkf",
    "observer", "ostruct", "pstore", "rdoc", "resolv", "rinda", "syslog",
];

fn requires_in(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            requires_in(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rb") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        for line in source.lines() {
            let line = line.trim();
            // Only unconditional top-level requires: a `require` inside
            // a `begin`/`rescue LoadError` is an optional dependency by
            // construction.
            let Some(rest) = line.strip_prefix("require \"") else { continue };
            let Some(name) = rest.split('"').next() else { continue };
            if BUNDLED_GEMS.contains(&name) {
                out.insert(name.to_string());
            }
        }
    }
}

#[test]
fn the_scaffold_gemfile_names_every_bundled_gem_the_runtime_requires() {
    // The whole ruby-family runtime, not just the scaffold: the
    // emitted tree ships both, and `base64` / `resolv` are required
    // from the runtime beside it.
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel");
    let root = runtime.join("scaffold");
    let mut required: BTreeSet<String> = BTreeSet::new();
    requires_in(&runtime, &mut required);
    assert!(
        !required.is_empty(),
        "the scaffold requires at least `bigdecimal`; the walk found nothing, so it is looking in the wrong place"
    );

    let gemfile = std::fs::read_to_string(root.join("Gemfile")).expect("scaffold Gemfile");
    let declared: BTreeSet<String> = gemfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("gem \"")?.split('"').next())
        .map(str::to_string)
        .collect();

    let missing: Vec<&String> = required.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "the emitted tree requires these bundled gems but its Gemfile does not name them, \
         so `bundle exec` cannot load them: {missing:?}"
    );
}
