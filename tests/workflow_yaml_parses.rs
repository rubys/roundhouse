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

#[cfg(unix)]
#[test]
fn campfire_comparisons_require_an_uploaded_binary_and_report_blocking() {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let producer = &workflow["jobs"]["build-campfire-compare-spinel"];
    let consumer = &workflow["jobs"]["campfire-compare-spinel"];
    assert_eq!(producer["continue-on-error"].as_bool(), Some(true));
    assert_eq!(consumer["continue-on-error"].as_bool(), Some(true));
    assert_eq!(
        consumer["needs"].as_str(),
        Some("build-campfire-compare-spinel")
    );
    assert_eq!(
        producer["outputs"]["artifact-id"].as_str(),
        Some("${{ steps.binary.outputs.artifact-id }}")
    );
    assert_eq!(
        consumer["if"].as_str(),
        Some(
            "${{ !cancelled() && needs.build-campfire-compare-spinel.outputs.artifact-id != '' }}"
        )
    );
    let steps = producer["steps"].as_sequence().unwrap();
    let upload = steps
        .iter()
        .find(|step| step["id"].as_str() == Some("binary"))
        .unwrap();
    assert!(upload["uses"]
        .as_str()
        .unwrap()
        .starts_with("actions/upload-artifact@"));
    assert_eq!(
        upload["with"]["name"].as_str(),
        Some("campfire-compare-spinel")
    );
    let matrix = consumer["strategy"]["matrix"]["include"]
        .as_sequence()
        .unwrap();
    let modes: Vec<_> = matrix
        .iter()
        .map(|entry| {
            (
                entry["gc"].as_str().unwrap(),
                entry["flag"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        modes,
        [
            ("default", ""),
            ("minor-gc", "--minor-gc"),
            ("verify-gen", "--verify-gen")
        ]
    );

    let report = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Report comparison availability"))
        .unwrap();
    assert_eq!(report["if"].as_str(), Some("${{ !cancelled() }}"));
    assert_eq!(
        report["env"]["ARTIFACT_ID"].as_str(),
        Some("${{ steps.binary.outputs.artifact-id }}")
    );
    for (artifact_id, expected) in [
        ("", "No Campfire comparison binary was uploaded. The default, minor-gc and verify-gen comparisons are blocked, not passed; see the producer failure above.\n"),
        ("12345", "Campfire comparison binary uploaded; default, minor-gc and verify-gen comparisons can run.\n"),
    ] {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let summary = std::env::temp_dir().join(format!("campfire-ready-{}-{unique}.md", std::process::id()));
        let output = Command::new("bash")
            .args(["-e", "-c", report["run"].as_str().unwrap()])
            .env("ARTIFACT_ID", artifact_id)
            .env("GITHUB_STEP_SUMMARY", &summary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fs::read_to_string(&summary).unwrap(), expected);
        fs::remove_file(summary).unwrap();
    }
}

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

#[test]
fn spinel_model_differential_does_not_wait_for_the_gc_comparison_build() {
    let src = fs::read_to_string(".github/workflows/ci.yml").expect("read CI workflow");
    let ci: serde_yaml_ng::Value = serde_yaml_ng::from_str(&src).expect("parse CI workflow");
    let jobs = &ci["jobs"];
    let db = &jobs["campfire-db-differential-spinel"];
    assert_eq!(db["needs"].as_str(), Some("build-spinel"));
    assert_eq!(db["continue-on-error"].as_bool(), Some(true));

    let command = "scripts/campfire-db-differential --spinel /tmp/campfire";
    let db_steps = db["steps"].as_sequence().expect("DB job steps");
    let runs: Vec<_> = db_steps
        .iter()
        .filter(|step| step["run"].as_str() == Some(command))
        .collect();
    assert_eq!(runs.len(), 1, "run the model differential exactly once");
    assert!(
        runs[0].get("if").is_none(),
        "do not gate it on a GC matrix value"
    );

    let gc = &jobs["campfire-compare-spinel"];
    assert_eq!(gc["needs"].as_str(), Some("build-campfire-compare-spinel"));
    let modes: Vec<_> = gc["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("GC matrix")
        .iter()
        .map(|mode| (mode["gc"].as_str().unwrap(), mode["flag"].as_str().unwrap()))
        .collect();
    assert_eq!(
        modes,
        [
            ("default", ""),
            ("minor-gc", "--minor-gc"),
            ("verify-gen", "--verify-gen")
        ]
    );
    assert!(
        gc["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .all(|step| step["run"].as_str() != Some(command))
    );
}
