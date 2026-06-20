use std::path::Path;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::{crystal, csharp, elixir, go, kotlin, python, rust, swift, typescript};
use roundhouse::ingest::ingest_app;
use roundhouse::profile::DeploymentProfile;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let target = take_flag(&mut args, "--target").unwrap_or_else(|| "typescript".into());
    let profile = take_flag(&mut args, "--profile");
    let out_override = take_flag(&mut args, "--out");
    let fixture = args.first().cloned().unwrap_or_else(|| "fixtures/real-blog".into());

    let mut app = ingest_app(Path::new(&fixture)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);

    let (files, default_out_dir) = match target.as_str() {
        "typescript" | "ts" => {
            // `--profile` is only meaningful for the typescript
            // target — other targets don't yet branch on profile.
            // `worker` (SharedWorker browser deployment) gets its
            // own out_dir so a typescript build doesn't clobber it.
            match profile.as_deref() {
                Some("worker") => (
                    typescript::emit_with_profile(&app, &DeploymentProfile::worker()),
                    "/tmp/rh-ts-worker-pass2",
                ),
                Some("node-async") => (
                    typescript::emit_with_profile(&app, &DeploymentProfile::node_async()),
                    "/tmp/rh-ts-async-pass2",
                ),
                Some("node-sync") => (
                    typescript::emit_with_profile(&app, &DeploymentProfile::node_sync()),
                    "/tmp/rh-ts-pass2",
                ),
                None => (typescript::emit(&app), "/tmp/rh-ts-pass2"),
                Some(other) => panic!(
                    "unknown profile: {other} (valid: node-sync, node-async, worker)"
                ),
            }
        }
        "crystal" | "cr" => (crystal::emit(&app), "/tmp/rh-cr-pass2"),
        "rust" | "rs" => (rust::emit(&app), "/tmp/rh-rs-pass2"),
        "python" | "py" => (python::emit(&app), "/tmp/rh-py-pass2"),
        "elixir" | "ex" => (elixir::emit(&app), "/tmp/rh-ex-pass2"),
        "go" => (go::emit(&app), "/tmp/rh-go-pass2"),
        "kotlin" | "kt" => (kotlin::emit(&app), "/tmp/rh-kt-pass2"),
        "swift" | "sw" => (swift::emit(&app), "/tmp/rh-swift-pass2"),
        "csharp" | "cs" => (csharp::emit(&app), "/tmp/rh-cs-pass2"),
        other => panic!("unknown target: {other}"),
    };

    // `--out <path>` overrides the per-target default. Used by test
    // harnesses (tests/browser_smoke) to keep emitted output under
    // their own controlled directory rather than the shared /tmp
    // path where two harnesses could clobber each other.
    let out_dir = out_override.as_deref().unwrap_or(default_out_dir);
    let out = Path::new(out_dir);
    if out.exists() { std::fs::remove_dir_all(out).ok(); }
    std::fs::create_dir_all(out).ok();
    for f in &files {
        let path = out.join(&f.path);
        if let Some(p) = path.parent() { std::fs::create_dir_all(p).ok(); }
        std::fs::write(&path, &f.content).expect("write");
    }
    println!("emitted {} files to {}", files.len(), out_dir);
}

/// Remove `--name <value>` from args and return the value, or None
/// if the flag isn't present.
fn take_flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    if i < args.len() { Some(args.remove(i)) } else { None }
}
