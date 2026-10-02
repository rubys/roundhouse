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
fn release_runs_only_for_version_tag_pushes() {
    let release: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/release.yml").unwrap())
            .unwrap();
    let events = release["on"].as_mapping().unwrap();
    assert_eq!(events.len(), 1, "no PR or manual publishing trigger");
    assert_eq!(
        release["on"]["push"]["tags"][0].as_str(),
        Some("**[0-9]+.[0-9]+.[0-9]+*")
    );
    assert!(release["on"]["push"].get("branches").is_none());
    let config = fs::read_to_string("dist-workspace.toml").unwrap();
    assert!(
        config.lines().any(|line| line == "pr-run-mode = \"skip\""),
        "regeneration must not restore the PR trigger"
    );
}

#[test]
fn pr_archives_remain_tested_without_pages_publication_work() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    assert_eq!(
        jobs["build-site"]["if"].as_str(),
        Some("${{ !cancelled() && needs.generate-fixture.result == 'success' && needs.build-wasm.result == 'success' }}")
    );
    assert_eq!(jobs["smoke"]["needs"].as_str(), Some("build-site"));
    let steps = jobs["build-site"]["steps"].as_sequence().unwrap();
    for (id, output, renderer) in [
        ("fetch-bench", "bench_data", "Render bench page"),
        (
            "fetch-lobsters-bench",
            "lobsters_bench_data",
            "Render lobsters bench page",
        ),
        (
            "fetch-lobsters-specs",
            "specs_data",
            "Render lobsters conformance page",
        ),
        (
            "fetch-campfire-bench",
            "campfire_bench",
            "Render campfire bench page",
        ),
        (
            "fetch-campfire-suite-spinel",
            "compiled_data",
            "Render compiled campfire conformance page",
        ),
    ] {
        let fetch = steps
            .iter()
            .find(|step| step["id"].as_str() == Some(id))
            .unwrap();
        assert_eq!(
            fetch["if"].as_str(),
            Some("github.ref == 'refs/heads/main'")
        );
        let render = steps
            .iter()
            .find(|step| step["name"].as_str() == Some(renderer))
            .unwrap();
        // A skipped fetch has no 'present' output, so its renderer must skip too.
        assert_eq!(
            render["if"].as_str(),
            Some(format!("steps.{id}.outputs.{output} == 'present'").as_str())
        );
    }
    let archives = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Upload browse archives"))
        .unwrap();
    assert!(archives.get("if").is_none(), "PR smoke needs the archives");
    assert_eq!(archives["with"]["name"].as_str(), Some("browse-archives"));
    let pages = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Upload Pages artifact"))
        .unwrap();
    assert_eq!(
        pages["if"].as_str(),
        Some("github.ref == 'refs/heads/main'")
    );
    // Keep the status function: a failed Campfire floor must not suppress
    // its explanatory publication, but cancellation or a bad site must.
    assert_eq!(
        jobs["assemble-site"]["if"].as_str(),
        Some("${{ !cancelled() && github.ref == 'refs/heads/main' && needs.build-site.result == 'success' }}")
    );
    assert_eq!(
        jobs["deploy"]["if"].as_str(),
        Some("github.ref == 'refs/heads/main'")
    );
}

#[test]
fn draft_transitions_replace_the_previous_pr_run() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let events = ci["on"]["pull_request"]["types"].as_sequence().unwrap();
    for event in ["ready_for_review", "converted_to_draft"] {
        assert!(events.iter().any(|value| value.as_str() == Some(event)));
    }
    assert_eq!(
        ci["concurrency"]["group"].as_str(),
        Some("${{ github.workflow }}-${{ github.event_name }}-${{ github.event.pull_request.number || github.ref }}")
    );
    assert_eq!(
        ci["concurrency"]["cancel-in-progress"].as_str(),
        Some("${{ github.event_name == 'pull_request' }}")
    );
}

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

#[cfg(unix)]
#[test]
fn campfire_failure_capture_keeps_the_original_exit_and_actual_c() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let job = &workflow["jobs"]["build-campfire-compare-spinel"];
    assert_eq!(job["continue-on-error"].as_bool(), Some(true));
    let steps = job["steps"].as_sequence().unwrap();
    let step = |name: &str| {
        steps
            .iter()
            .find(|step| step["name"].as_str() == Some(name))
            .unwrap()
    };
    let build = step("Emit and build the comparison binary");
    assert_eq!(build["id"].as_str(), Some("build"));
    let capture = step("Capture Campfire compiler failure");
    let upload = step("Upload Campfire compiler failure");
    for step in [capture, upload] {
        assert_eq!(
            step["if"].as_str(),
            Some("failure() && steps.build.outcome == 'failure'")
        );
    }
    assert_eq!(
        upload["with"]["name"].as_str(),
        Some("campfire-compiler-repro")
    );
    assert_eq!(upload["with"]["path"].as_str(), Some("ci-campfire-repro"));
    assert_eq!(upload["with"]["retention-days"].as_u64(), Some(7));

    // Execute the actual workflow bodies with controlled emit/build exits.
    // tee must not hide either failure; missing C must not select stale C.
    for (emit_exit, build_exit, reported_c, retained_c) in [
        (31, 0, false, false),
        (0, 47, true, true),
        (0, 47, false, false),
        (0, 47, true, false),
        (0, 0, false, false),
    ] {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("campfire-ci {}-{unique}", std::process::id()));
        let source = root.join("build/campfire-compare-spinel");
        fs::create_dir_all(source.join("app/models")).unwrap();
        fs::create_dir_all(source.join("sig")).unwrap();
        fs::create_dir_all(source.join("build")).unwrap();
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::write(source.join("bin/blog.rb"), "require_relative '../main'\n").unwrap();
        fs::write(source.join("main.rb"), "require_relative 'boot'\n").unwrap();
        fs::write(
            source.join("boot.rb"),
            "require_relative 'app/models/message'\n",
        )
        .unwrap();
        fs::write(source.join("app/models/message.rb"), "class Message; end\n").unwrap();
        fs::write(source.join("sig/message.rbs"), "class Message\nend\n").unwrap();
        fs::write(source.join("build/blog"), "not a diagnostic input").unwrap();
        if retained_c {
            fs::write(root.join("actual.c"), "int actual_failure;\n").unwrap();
        }
        fs::write(root.join("stale.c"), "int unrelated;\n").unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        fs::create_dir(root.join("spinel-dist")).unwrap();
        fs::write(
            root.join("spinel-dist/revision.txt"),
            "exact-spinel-revision\n",
        )
        .unwrap();
        for (path, body) in [
            ("bin/cargo", "#!/bin/sh\necho emit-output >&2\nexit \"$EMIT_EXIT\"\n"),
            (
                "bin/make",
                "#!/bin/sh\necho build-output >&2\nif [ \"$RETAINED_C\" = true ]; then\n  echo \"spinel: the generated C is kept at $PWD/actual.c\" >&2\nfi\necho \"note: the generated C is kept at $PWD/stale.c\"\nexit \"$BUILD_EXIT\"\n",
            ),
            ("spinel-dist/spinel", "#!/bin/sh\n[ \"$1\" = --version ] || exit 99\necho compiler-version\n"),
        ] {
            let path = root.join(path);
            fs::write(&path, body).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut command = Command::new("bash");
        command
            .current_dir(&root)
            .args(["-e", "-c", build["run"].as_str().unwrap()]);
        command.env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        );
        command.env("EMIT_EXIT", emit_exit.to_string());
        command.env("BUILD_EXIT", build_exit.to_string());
        command.env("RETAINED_C", reported_c.to_string());
        let output = command.output().unwrap();
        let expected_exit = if emit_exit != 0 {
            emit_exit
        } else {
            build_exit
        };
        assert_eq!(output.status.code(), Some(expected_exit), "{output:?}");
        assert_eq!(
            root.join("campfire-compare-spinel.tar.gz").exists(),
            expected_exit == 0
        );
        assert_eq!(
            fs::read_to_string(root.join("campfire-emit.log")).unwrap(),
            "emit-output\n"
        );
        assert_eq!(root.join("campfire-build.log").exists(), emit_exit == 0);
        if expected_exit != 0 {
            let output = Command::new("bash")
                .current_dir(&root)
                .args(["-e", "-c", capture["run"].as_str().unwrap()])
                .env("GITHUB_SHA", "roundhouse-head")
                .env("CAMPFIRE_SHA", "pinned-campfire")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let bundle = root.join("ci-campfire-repro");
            let versions = fs::read_to_string(bundle.join("versions.txt")).unwrap();
            assert!(versions.contains("Roundhouse: roundhouse-head\nCampfire: pinned-campfire\n"));
            assert!(versions.contains("Spinel revision: exact-spinel-revision\ncompiler-version\n"));
            assert_eq!(
                fs::read_to_string(bundle.join("campfire-emit.log")).unwrap(),
                "emit-output\n"
            );
            if emit_exit == 0 {
                let diagnostic = if reported_c {
                    format!(
                        "spinel: the generated C is kept at {}/actual.c\n",
                        root.display()
                    )
                } else {
                    String::new()
                };
                assert_eq!(
                    fs::read_to_string(bundle.join("campfire-build.log")).unwrap(),
                    format!(
                        "build-output\n{diagnostic}note: the generated C is kept at {}/stale.c\n",
                        root.display()
                    )
                );
            }
            assert_eq!(
                fs::read_to_string(bundle.join("sources/bin/blog.rb")).unwrap(),
                "require_relative '../main'\n"
            );
            assert_eq!(
                fs::read_to_string(bundle.join("sources/main.rb")).unwrap(),
                "require_relative 'boot'\n"
            );
            assert_eq!(
                fs::read_to_string(bundle.join("sources/boot.rb")).unwrap(),
                "require_relative 'app/models/message'\n"
            );
            assert_eq!(
                fs::read_to_string(bundle.join("sources/app/models/message.rb")).unwrap(),
                "class Message; end\n"
            );
            assert_eq!(
                fs::read_to_string(bundle.join("sources/sig/message.rbs")).unwrap(),
                "class Message\nend\n"
            );
            assert!(!bundle.join("sources/build").exists());
            assert_eq!(bundle.join("generated.c").exists(), retained_c);
            if retained_c {
                assert_eq!(
                    fs::read_to_string(bundle.join("generated.c")).unwrap(),
                    "int actual_failure;\n"
                );
            } else {
                assert!(bundle.join("capture.txt").is_file());
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
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
