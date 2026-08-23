//! Every file under `.github/workflows/` is valid YAML.
//!
//! A workflow that does not PARSE fails in a way no other gate can see:
//! GitHub reports "This run likely failed because of a workflow file
//! issue", the run has ZERO jobs, and every job-by-job check — the one
//! this project relies on, because an advisory job's red hides inside a
//! green run conclusion — has nothing to read. The whole run is a single
//! red X with no test, no toolchain, and no floor behind it.
//!
//! Measured: `run: "$GITHUB_WORKSPACE/scripts/ci-apt-install"
//! libvips-dev`. A scalar that OPENS with a quote is a quoted scalar,
//! and YAML has nowhere to put the words after the closing quote. Every
//! other call to that script in this file sits inside a `run: |` block,
//! which is why the shape looked right.

use std::fs;
use std::path::Path;

#[test]
fn every_workflow_file_parses_as_yaml() {
    let dir = Path::new(".github/workflows");
    let mut checked = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir).expect("read .github/workflows") {
        let path = entry.expect("dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "yml" && ext != "yaml" {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        checked += 1;
        match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&src) {
            Ok(v) => {
                // A workflow with no `jobs:` mapping parses but runs
                // nothing — the same zero-job outcome by another route.
                let jobs = v.get("jobs").and_then(|j| j.as_mapping());
                match jobs {
                    Some(m) if !m.is_empty() => {}
                    _ => errors.push(format!("{}: no jobs declared", path.display())),
                }
            }
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(checked > 0, "no workflow files found under {dir:?}");
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
