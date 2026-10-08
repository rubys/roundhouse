//! Opt-in MRI ISeq delivery. Prepare only after source markers and bundled
//! requires are finalized; compile with the very gem vendored into the output.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Existing source delivery remains the default; the legacy alias is accepted.
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

// Compile application and runtime units, but do not eagerly evaluate them.
// The original boot/entry points retain their stdlib requires, assignments,
// conditionals and load order. Seeds, tools, tests and config remain source.
fn compile_as_unit(path: &str) -> bool {
    path.ends_with(".rb") && (path.starts_with("app/") || path.starts_with("runtime/"))
}

pub fn prepare(files: &mut Vec<(String, String)>) -> Result<(), String> {
    if !enabled() || files.iter().any(|(path, _)| path == "units.json") {
        return Ok(());
    }
    let mut by_path: BTreeMap<String, String> = files.iter().cloned().collect();
    let boot = by_path
        .get_mut("boot.rb")
        .ok_or("roundsnap: CRuby boot.rb missing")?;
    // Install before the unchanged boot chain; never compile a second boot
    // or replace it with an eager walk through all manifest entries.
    boot.insert_str(0, "require_relative \"vendor/roundsnap/lib/roundsnap\"\nRoundsnap::Loader.install!(root: __dir__)\n");

    let units: Vec<_> = by_path
        .iter()
        .filter(|(path, _)| compile_as_unit(path))
        .map(|(path, source)| {
            serde_json::json!({
                "key": path.strip_suffix(".rb").unwrap(),
                "source": source,
                "file": path,
                "first_lineno": 1,
            })
        })
        .collect();
    by_path.insert(
        "units.json".into(),
        serde_json::to_string_pretty(&units).map_err(|e| e.to_string())? + "\n",
    );
    for (path, source) in GEM_FILES {
        by_path.insert(format!("vendor/roundsnap/{path}"), (*source).into());
    }
    let gemfile = by_path
        .get_mut("Gemfile")
        .ok_or("roundsnap: CRuby Gemfile missing")?;
    gemfile.push_str("\n# MRI ISeq delivery\ngem \"roundsnap\", path: \"vendor/roundsnap\"\n");
    by_path.remove("Gemfile.lock");
    if !keep_source() {
        by_path.retain(|path, _| !compile_as_unit(path));
    }
    *files = by_path.into_iter().collect();
    Ok(())
}

/// Publish a compiled generation without deleting the previous manifest or
/// binaries. The presence of the prepared input, not a process-global flag,
/// determines whether there is work to do.
pub fn finalize(dest: &Path) -> Result<(), String> {
    let units = dest.join("units.json");
    if !units.is_file() {
        return Ok(());
    }
    let exe = dest.join("vendor/roundsnap/exe/roundsnap-compile");
    let output = Command::new("ruby")
        .arg(&exe)
        .arg("--out")
        .arg(dest)
        .arg("--units")
        .arg(&units)
        .output()
        .map_err(|e| format!("roundsnap: spawn ruby {}: {e}", exe.display()))?;
    if !output.status.success() {
        return Err(format!(
            "roundsnap: compiler failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // write_to_dir preserves unlisted files. Remove only successfully
    // compiled units, including sources left by an earlier plain-Ruby emit.
    if !keep_source() {
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dest.join("manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for key in manifest["units"]
            .as_object()
            .ok_or("roundsnap: manifest units missing")?
            .keys()
        {
            let path = dest.join(format!("{key}.rb"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("roundsnap: remove {}: {e}", path.display())),
            }
        }
    }
    std::fs::remove_file(units).map_err(|e| format!("roundsnap: remove units.json: {e}"))?;
    Ok(())
}

// Compile-time embedding, like the runtime/scaffold table. Installed binaries
// need neither the build checkout nor a CWD-relative gem. Invoke its vendored
// executable through MRI, since write_to_dir writes text without execute bits.
const GEM_FILES: &[(&str, &str)] = &[
    (
        "roundsnap.gemspec",
        include_str!("../gems/roundsnap/roundsnap.gemspec"),
    ),
    ("README.md", include_str!("../gems/roundsnap/README.md")),
    ("LICENSE", include_str!("../gems/roundsnap/LICENSE")),
    (
        "lib/roundsnap.rb",
        include_str!("../gems/roundsnap/lib/roundsnap.rb"),
    ),
    (
        "lib/roundsnap/version.rb",
        include_str!("../gems/roundsnap/lib/roundsnap/version.rb"),
    ),
    (
        "lib/roundsnap/compiler.rb",
        include_str!("../gems/roundsnap/lib/roundsnap/compiler.rb"),
    ),
    (
        "lib/roundsnap/loader.rb",
        include_str!("../gems/roundsnap/lib/roundsnap/loader.rb"),
    ),
    (
        "lib/roundsnap/source_map.rb",
        include_str!("../gems/roundsnap/lib/roundsnap/source_map.rb"),
    ),
    (
        "exe/roundsnap-compile",
        include_str!("../gems/roundsnap/exe/roundsnap-compile"),
    ),
];
