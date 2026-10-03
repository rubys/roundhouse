use std::fs;

#[test]
fn unit_separates_build_timing_without_reducing_coverage() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let unit = &ci["jobs"]["unit"];
    assert!(unit.get("if").is_none());
    assert!(unit.get("continue-on-error").is_none());
    assert_eq!(unit["runs-on"].as_str(), Some("ubuntu-latest"));
    assert_eq!(
        unit["env"]["CARGO_PROFILE_TEST_SPLIT_DEBUGINFO"].as_str(),
        Some("unpacked")
    );
    assert!(ci["env"].get("CARGO_PROFILE_TEST_SPLIT_DEBUGINFO").is_none());
    let steps = unit["steps"].as_sequence().unwrap();
    let build = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Build all test targets"))
        .expect("compile every target with timings");
    let run = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Run all test targets"))
        .expect("execute every non-ignored test, not just compile it");
    assert!(build < run);
    for (index, phase, command) in [
        (
            build,
            "build",
            "cargo test --locked --all-targets --no-run --timings",
        ),
        (run, "tests", "cargo test --locked --all-targets"),
    ] {
        assert!(steps[index].get("if").is_none());
        assert!(steps[index].get("continue-on-error").is_none());
        let body = steps[index]["run"].as_str().unwrap();
        assert!(body.contains(&format!("--out \"$RUNNER_TEMP/unit-resources/{phase}\" --")));
        assert!(body.trim_end().ends_with(command));
    }
    let timings = steps
        .iter()
        .find(|step| step["with"]["name"].as_str() == Some("unit-build-timings"))
        .expect("retain build timings for investigation");
    assert_eq!(timings["if"].as_str(), Some("always()"));
    assert_eq!(
        timings["with"]["path"].as_str(),
        Some("target/cargo-timings/")
    );
    let bench = steps
        .iter()
        .find(|step| {
            step["name"].as_str()
                == Some("Emit every bench lane in the debug profile (scripts/bench's shape)")
        })
        .expect("retain the independent dev-profile stack-overflow gate");
    assert!(bench.get("if").is_none());
    assert!(bench.get("continue-on-error").is_none());
    let body = bench["run"].as_str().unwrap();
    assert!(body.contains("bash -euo pipefail -c"));
    assert!(body.contains("typescript crystal rust python elixir go kotlin swift csharp"));
    assert!(body.contains("cargo run --quiet --bin emit_preview -- --target"));
    let resources = steps
        .iter()
        .find(|step| step["with"]["name"].as_str() == Some("unit-resources"))
        .expect("retain phase samples even when a command fails");
    assert_eq!(resources["if"].as_str(), Some("always()"));
    assert_eq!(
        resources["with"]["path"].as_str(),
        Some("${{ runner.temp }}/unit-resources/")
    );
}

#[test]
#[cfg(all(target_os = "linux", debug_assertions))]
fn test_backtraces_retain_library_and_integration_source_locations() {
    const PROBE: &str = "ROUNDHOUSE_TEST_BACKTRACE_PROBE";
    if std::env::var_os(PROBE).is_some() {
        // The child runs outside the checkout: this deliberately panics in
        // first-party library code, with an integration-test frame above it.
        roundhouse::fixtures::real_blog();
        panic!("missing-fixture probe unexpectedly returned");
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "roundhouse-backtrace-{}-{unique}", std::process::id()
    ));
    fs::create_dir(&root).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "test_backtraces_retain_library_and_integration_source_locations",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .env("RUST_BACKTRACE", "1")
        .current_dir(&root)
        .output()
        .unwrap();
    fs::remove_dir(&root).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(101), "{stderr}");
    for source in ["src/fixtures.rs:", "tests/ci_policy_workflow.rs:"] {
        // The panic header includes a location even without debug info.
        // Require symbolicated stack frames, not just that header.
        assert!(
            stderr.lines().any(|line| line.trim_start().starts_with("at ") && line.contains(source)),
            "missing file/line backtrace for {source}:\n{stderr}"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn resource_monitor_preserves_failures_and_metric_meanings() {
    let result = std::process::Command::new("python3")
        .args(["-B", "tests/ci_resources_test.py", "-v"])
        .output()
        .expect("CI helper tests require python3");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn routing_and_required_results_reject_false_green() {
    for test in ["tests/ci_plan_test.py", "tests/ci_archive_evidence_test.py"] {
        let result = std::process::Command::new("python3")
            .args(["-B", test, "-v"])
            .output()
            .expect("CI helper tests require python3");
        assert!(
            result.status.success(),
            "{test}:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn compact_and_extra_compare_share_commands_but_not_results() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    assert_eq!(
        jobs["compare"]["strategy"]["matrix"]["target"],
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>("[rust, typescript]").unwrap()
    );
    assert_eq!(jobs["compare"]["steps"], jobs["compare-extra"]["steps"]);
    assert_eq!(
        jobs["compare-extra"]["strategy"]["max-parallel"].as_u64(),
        Some(2)
    );
    assert_eq!(jobs["smoke"]["strategy"]["max-parallel"].as_u64(), Some(2));
    let smoke_guard = jobs["smoke"]["if"].as_str().unwrap();
    for condition in [
        "!cancelled()",
        "needs.plan.result == 'success'",
        "needs.build-site.result == 'success'",
    ] {
        assert!(
            smoke_guard.contains(condition),
            "selected smoke must run after its skipped WASM ancestor: {condition}"
        );
    }
    assert_eq!(
        jobs["campfire-compare-spinel"]["strategy"]["max-parallel"].as_u64(),
        Some(1)
    );
    assert!(
        ci["on"]["pull_request"].get("paths-ignore").is_none(),
        "summary must run even for documentation-only PRs"
    );
    assert!(jobs.get("ci-required").is_none());
    assert_eq!(jobs["ci-summary"]["name"].as_str(), Some("CI summary"));
    assert_eq!(
        ci["on"]["workflow_call"]["outputs"]["complete"]["value"].as_str(),
        Some("${{ jobs.ci-summary.outputs.complete }}")
    );
    for name in ["compact-required", "ci-summary"] {
        assert_eq!(jobs[name]["if"].as_str(), Some("always()"));
    }
    let gate = jobs["ci-summary"]["needs"].as_sequence().unwrap();
    for name in jobs.as_mapping().unwrap().keys().filter_map(|v| v.as_str()) {
        if name != "ci-summary" {
            assert!(
                gate.iter().any(|v| v.as_str() == Some(name)),
                "missing result: {name}"
            );
        }
    }
    assert_eq!(ci["permissions"]["contents"].as_str(), Some("read"));
    assert!(ci["permissions"].get("pages").is_none());
    assert!(ci["permissions"].get("id-token").is_none());
}

#[cfg(unix)]
#[test]
fn focused_framework_loop_runs_every_selection_and_preserves_failure() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let step = ci["jobs"]["framework-tests-spinel"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Run selected native framework checks"))
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("framework-loop-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let cargo = root.join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CARGO_LOG\"\n[ \"$3\" != fails ]\n",
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    for (tests, expected_success, expected_log) in [
        (
            "first second",
            true,
            "test --test first -- --ignored --nocapture\ntest --test second -- --ignored --nocapture\n",
        ),
        (
            "fails survivor",
            false,
            "test --test fails -- --ignored --nocapture\ntest --test survivor -- --ignored --nocapture\n",
        ),
        ("", false, ""),
    ] {
        let log = root.join("cargo.log");
        fs::write(&log, "").unwrap();
        let result = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", step["run"].as_str().unwrap()])
            .env("TESTS", tests)
            .env("CARGO_LOG", &log)
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(result.status.success(), expected_success, "{result:?}");
        assert_eq!(fs::read_to_string(log).unwrap(), expected_log);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn spinel_jobs_are_selected_explicitly_and_archive_evidence_reaches_pages() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    for name in [
        "framework-tests-spinel",
        "campfire-db-differential-spinel",
        "toolchain-spinel",
        "compare-spinel",
        "smoke-spinel",
        "smoke-campfire",
    ] {
        let job = &jobs[name];
        let needs = job["needs"]
            .as_sequence()
            .expect("Spinel consumer needs plan and producer");
        assert!(
            needs.iter().any(|need| need.as_str() == Some("plan")),
            "{name}"
        );
        assert!(
            job["if"].as_str().unwrap().contains(&format!(
                "contains(fromJSON(needs.plan.outputs.jobs), '{name}')"
            )),
            "{name}"
        );
    }

    for (job_name, artifact_name) in [
        ("build-site", "browse-archives"),
        ("build-campfire-archive", "campfire-archive"),
    ] {
        let upload = jobs[job_name]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["with"]["name"].as_str() == Some(artifact_name))
            .unwrap();
        assert_eq!(upload["if"].as_str(), Some("always()"));
        assert!(upload["with"]["path"].as_str().unwrap().ends_with("*.tgz"));
    }
    let report = &jobs["archive-results"];
    for dependency in [
        "build-site",
        "build-campfire-archive",
        "smoke",
        "smoke-spinel",
        "smoke-campfire",
        "smoke-campfire-docker",
    ] {
        assert!(
            report["needs"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|need| need.as_str() == Some(dependency))
        );
    }
    let report_steps = report["steps"].as_sequence().unwrap();
    assert_eq!(
        report_steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Collect this run's archive evidence"))
            .unwrap()["with"]["pattern"]
            .as_str(),
        Some("ci-archive-*")
    );
    assert_eq!(
        report_steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Save archive outcome report"))
            .unwrap()["with"]["name"]
            .as_str(),
        Some("archive-results")
    );

    let assemble = &jobs["assemble-site"];
    assert!(
        assemble["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|need| need.as_str() == Some("archive-results"))
    );
    let steps = assemble["steps"].as_sequence().unwrap();
    let verify = steps.iter().position(|step| step["run"].as_str() == Some("python3 scripts/ci-archive-evidence.py verify --root _site --report _site/ci/archive-results.json")).expect("archive verification step");
    let pages = steps
        .iter()
        .position(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|uses| uses.starts_with("actions/upload-pages-artifact@"))
        })
        .unwrap();
    assert!(verify < pages);
    assert!(
        jobs["ci-summary"]["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|need| need.as_str() == Some("archive-results"))
    );
}

#[test]
fn full_scheduler_runs_every_preflight_success_fresh_and_never_grants_pr_deploy_permissions() {
    let full: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/full-ci.yml").unwrap())
            .unwrap();
    assert_eq!(
        full["on"]["schedule"][0]["cron"].as_str(),
        Some("17 */4 * * *")
    );
    assert!(full["on"].get("push").is_none());
    assert!(full["on"].get("pull_request").is_none());
    assert_eq!(
        full["concurrency"]["cancel-in-progress"].as_bool(),
        Some(false)
    );
    let jobs = &full["jobs"];
    let preflight = &jobs["preflight"];
    assert!(preflight["outputs"].get("run").is_none());
    assert!(preflight["outputs"].get("known").is_none());
    assert!(jobs.get("checkpoint").is_none());
    let preflight_text = serde_yaml_ng::to_string(preflight).unwrap();
    assert!(!preflight_text.contains("actions/cache"));
    assert_eq!(
        preflight_text
            .matches("repos/matz/spinel/commits/master")
            .count(),
        1
    );
    assert_eq!(
        jobs["validation"]["uses"].as_str(),
        Some("./.github/workflows/ci.yml")
    );
    assert_eq!(jobs["validation"]["with"]["full"].as_bool(), Some(true));
    assert_eq!(
        jobs["validation"]["permissions"]["contents"].as_str(),
        Some("read")
    );
    assert_eq!(
        jobs["validation"]["permissions"]["actions"].as_str(),
        Some("read")
    );
    assert_eq!(
        jobs["validation"]["permissions"]
            .as_mapping()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(full["permissions"]["contents"].as_str(), Some("read"));
    let deploy = &jobs["deploy"];
    let guard = deploy["if"].as_str().unwrap();
    assert!(guard.contains("needs.validation.outputs.publication-ready == 'true'"));
    assert!(
        !guard.contains("needs.validation.result == 'success'"),
        "extra failures cannot hide repro publication"
    );
    assert!(deploy.get("continue-on-error").is_none());
    assert_eq!(deploy["permissions"]["pages"].as_str(), Some("write"));
    assert!(
        deploy["steps"][0]["run"]
            .as_str()
            .unwrap()
            .contains("$VALIDATED_SHA")
    );
}

#[cfg(unix)]
#[test]
fn scheduler_preflight_executes_publication_guards_and_unknown_input_fallback() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let full: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/full-ci.yml").unwrap())
            .unwrap();
    let body = full["jobs"]["preflight"]["steps"][0]["run"]
        .as_str()
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("full-preflight-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    for (name, script) in [(
        "gh",
        "#!/bin/sh\nprintf 'called\\n' >> \"$GH_LOG\"\n[ \"$MOCK_SPINEL\" != unavailable ] || exit 1\nprintf '%s\\n' \"$MOCK_SPINEL\"\n",
    )] {
        let path = root.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let spinel_sha = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
    for (event, repo, reference, publish, revision, expected) in [
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            spinel_sha,
            Some(("true", spinel_sha)),
        ),
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            "unavailable",
            Some(("true", "master")),
        ),
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            "malformed",
            Some(("true", "master")),
        ),
        (
            "workflow_dispatch",
            "contributor/roundhouse",
            "refs/heads/topic",
            "false",
            spinel_sha,
            Some(("false", spinel_sha)),
        ),
        (
            "workflow_dispatch",
            "rubys/roundhouse",
            "refs/heads/main",
            "true",
            spinel_sha,
            Some(("true", spinel_sha)),
        ),
        (
            "workflow_dispatch",
            "rubys/roundhouse",
            "refs/heads/topic",
            "true",
            spinel_sha,
            None,
        ),
    ] {
        let outputs = root.join("outputs");
        fs::write(&outputs, "").unwrap();
        let result = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", body])
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .env("EVENT", event)
            .env("GITHUB_REPOSITORY", repo)
            .env("GITHUB_REF", reference)
            .env("REQUEST_PUBLISH", publish)
            .env("MOCK_SPINEL", revision)
            .env("GH_LOG", root.join("gh.log"))
            .env("GITHUB_OUTPUT", &outputs)
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            expected.is_some(),
            "{event} {repo} {reference}: {result:?}"
        );
        let actual = fs::read_to_string(outputs).unwrap();
        if let Some((published, resolved)) = expected {
            assert_eq!(actual, format!("spinel={resolved}\npublish={published}\n"));
        } else {
            assert!(
                actual.is_empty(),
                "rejected publication must not issue outputs"
            );
        }
    }
    assert_eq!(
        fs::read_to_string(root.join("gh.log"))
            .unwrap()
            .lines()
            .count(),
        5
    );
    fs::remove_dir_all(root).unwrap();
}
