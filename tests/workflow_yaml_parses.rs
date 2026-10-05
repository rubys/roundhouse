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
fn spinel_cache_download_failure_falls_back_without_hiding_build_failures() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let job = &workflow["jobs"]["build-spinel"];
    assert_eq!(job["continue-on-error"].as_bool(), Some(true));
    let steps = job["steps"].as_sequence().unwrap();
    let step = |name: &str| {
        steps
            .iter()
            .find(|step| step["name"].as_str() == Some(name))
            .unwrap()
    };
    let setup = step("Set up sccache");
    assert_eq!(setup["id"].as_str(), Some("sccache"));
    assert_eq!(setup["continue-on-error"].as_bool(), Some(true));
    assert_eq!(setup["with"]["disable_annotations"].as_bool(), Some(true));
    let fallback = step("Build without compiler cache when installation fails");
    // outcome, not conclusion: continue-on-error makes conclusion 'success'.
    assert_eq!(
        fallback["if"].as_str(),
        Some("steps.sccache.outcome != 'success'")
    );
    for name in [
        "Restore Spinel compiler cache",
        "Start sccache server",
        "Show compiler cache statistics",
    ] {
        assert_eq!(
            step(name)["if"].as_str(),
            Some("steps.sccache.outcome == 'success'")
        );
    }
    let deps = step("make deps (fetch vendored libprism)");
    let build = step("make all");
    for required in [
        deps,
        build,
        step("Stage artifact tree"),
        step("Upload spinel toolchain"),
    ] {
        assert!(required.get("continue-on-error").is_none());
        assert!(
            required.get("if").is_none(),
            "cache failure must not skip the build"
        );
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("spinel-cache-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let env_file = root.join("github-env");
    let summary = root.join("summary");
    let output = Command::new("bash")
        .args([
            "-e",
            "-o",
            "pipefail",
            "-c",
            fallback["run"].as_str().unwrap(),
        ])
        .env("GITHUB_ENV", &env_file)
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&env_file).unwrap(), "NO_CCACHE=1\n");
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("::warning::"));
    assert!(fs::read_to_string(summary)
        .unwrap()
        .contains("without compiler cache"));

    // Execute the actual build bodies with controlled make exits, both with
    // caching enabled and after sourcing the fallback's exported environment.
    let make = root.join("make");
    fs::write(
        &make,
        r#"#!/bin/sh
[ "$1 $2" = '-C spinel-src' ] || exit 99
printf '%s:%s\n' "$3" "${NO_CCACHE:-cached}" >> "$MAKE_LOG"
case "$3" in
  deps) exit "$DEPS_EXIT" ;;
  all) exit "$BUILD_EXIT" ;;
  *) exit 98 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&make, fs::Permissions::from_mode(0o755)).unwrap();
    for cached in [true, false] {
        for (deps_exit, build_exit, expected_exit) in [(0, 0, 0), (31, 0, 31), (0, 47, 47)] {
            let log = root.join("make.log");
            fs::write(&log, "").unwrap();
            let body = format!(
                "{}\n{}\n{}",
                if cached {
                    ""
                } else {
                    "set -a; source \"$GITHUB_ENV\"; set +a"
                },
                deps["run"].as_str().unwrap(),
                build["run"].as_str().unwrap()
            );
            let output = Command::new("bash")
                .args(["-e", "-o", "pipefail", "-c", &body])
                .env(
                    "PATH",
                    format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
                )
                .env_remove("NO_CCACHE")
                .env("GITHUB_ENV", &env_file)
                .env("MAKE_LOG", &log)
                .env("DEPS_EXIT", deps_exit.to_string())
                .env("BUILD_EXIT", build_exit.to_string())
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(expected_exit), "{output:?}");
            let mode = if cached { "cached" } else { "1" };
            let expected = if deps_exit == 0 {
                format!("deps:{mode}\nall:{mode}\n")
            } else {
                format!("deps:{mode}\n")
            };
            assert_eq!(fs::read_to_string(log).unwrap(), expected);
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn campfire_docker_recipe_avoids_a_frontend_pull_and_ships_executable_boot() {
    use std::os::unix::fs::PermissionsExt;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("campfire-docker-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let result = std::process::Command::new("bash")
        .arg("scripts/campfire-docker-files")
        .arg(&root)
        .args(["roundhouse-sha", "campfire-sha", "spinel-version"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    let dockerfile = fs::read_to_string(root.join("Dockerfile")).unwrap();
    assert!(!dockerfile.lines().any(|line| line.starts_with("# syntax=")));
    assert!(dockerfile.contains("COPY boot ./boot\n"));
    assert!(!dockerfile.contains("--chmod="));
    assert_eq!(
        fs::metadata(root.join("boot"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn campfire_docker_smoke_caches_apt_for_eight_hours_and_always_builds() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let job = &workflow["jobs"]["smoke-campfire-docker"];
    let steps = job["steps"].as_sequence().unwrap();
    let step = |name: &str| {
        steps
            .iter()
            .find(|step| step["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("missing step {name}"))
    };

    // job-level env cannot use the runner context (actionlint / GH docs).
    // Cache keys may still use runner.os / runner.arch in steps.with.
    if let Some(env) = job.get("env").and_then(|v| v.as_mapping()) {
        for (key, value) in env {
            let name = key.as_str().unwrap_or("");
            let text = value.as_str().unwrap_or("");
            assert!(
                !text.contains("runner."),
                "{name} job env must not reference runner context; got {text}"
            );
        }
    }

    let window = step("Campfire Docker apt cache window");
    assert_eq!(window["id"].as_str(), Some("apt-window"));
    let window_run = window["run"].as_str().unwrap();
    assert!(window_run.contains("/ 28800"));
    assert!(
        window_run.contains("CAMPFIRE_DOCKER_CACHE=$RUNNER_TEMP/campfire-docker-buildkit"),
        "cache path must come from $RUNNER_TEMP via GITHUB_ENV"
    );

    let restore = step("Restore Campfire Docker apt layers");
    assert_eq!(restore["id"].as_str(), Some("docker-cache"));
    assert_eq!(restore["continue-on-error"].as_bool(), Some(true));
    assert_eq!(restore["uses"].as_str(), Some("actions/cache/restore@v6"));
    assert_eq!(
        restore["with"]["path"].as_str(),
        Some("${{ env.CAMPFIRE_DOCKER_CACHE }}")
    );
    let restore_key = restore["with"]["key"].as_str().unwrap();
    assert!(restore_key.contains("campfire-docker-apt-"));
    assert!(restore_key.contains("steps.apt-window.outputs.bucket"));
    assert!(
        restore["with"].get("restore-keys").is_none(),
        "no cross-bucket restore-keys: a miss must re-resolve apt"
    );

    let smoke = step("Build and run the image");
    let script = smoke["run"].as_str().unwrap();
    assert!(
        script.contains(r#"docker buildx build --load -t campfire "${cache_args[@]}" ."#),
        "image must still be tagged campfire for docker run (README install)"
    );
    assert!(
        script.contains("--driver docker-container")
            && script.contains("docker buildx use campfire-docker-cache"),
        "docker-container builder is required for type=local export on hosted runners"
    );
    assert!(script.contains("--cache-from"));
    assert!(script.contains("--cache-to"));
    assert!(script.contains("mode=max"));
    assert!(
        script.contains("ignore-error=true"),
        "cache export failure must not abort HTTP checks"
    );
    assert!(
        script.contains("GET /first_run") && script.contains("GET /account/logo"),
        "HTTP checks must always run"
    );
    assert!(
        !script.contains("sccache") && !script.contains("CCACHE"),
        "do not hide the pack compile behind a compiler cache"
    );

    let save = step("Save Campfire Docker apt layers");
    assert_eq!(save["continue-on-error"].as_bool(), Some(true));
    assert_eq!(save["uses"].as_str(), Some("actions/cache/save@v6"));
    assert_eq!(
        save["if"].as_str(),
        Some("steps.smoke.outcome == 'success' && steps.docker-cache.outputs.cache-hit != 'true'")
    );
    assert_eq!(save["with"]["key"].as_str(), Some(restore_key));

    for step in steps {
        let uses = step["uses"].as_str().unwrap_or("");
        assert!(
            !uses.contains("setup-buildx") && !uses.contains("build-push-action"),
            "local BuildKit cache under actions/cache; no build-push-action GHA backend"
        );
    }
}

#[test]
fn rust_ci_uses_the_repository_pin_before_restoring_caches() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    for (name, job) in workflow["jobs"].as_mapping().unwrap() {
        let steps = job["steps"].as_sequence().unwrap();
        let setup = steps
            .iter()
            .position(|step| step["uses"].as_str() == Some("./.github/actions/setup-rust"));
        for (i, step) in steps.iter().enumerate() {
            let uses = step["uses"].as_str().unwrap_or("");
            assert!(
                !uses.starts_with("dtolnay/rust-toolchain@"),
                "{name:?}: Rust must come from the repository pin"
            );
            if uses.starts_with("Swatinem/rust-cache@") {
                assert!(
                    setup.is_some_and(|setup| setup < i),
                    "{name:?}: select pinned Rust before caching"
                );
            }
        }
    }
    let wasm_setup = workflow["jobs"]["build-wasm"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["uses"].as_str() == Some("./.github/actions/setup-rust"))
        .unwrap();
    assert_eq!(
        wasm_setup["with"]["targets"].as_str(),
        Some("wasm32-wasip1")
    );
    let smoke_setup = workflow["jobs"]["smoke"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["uses"].as_str() == Some("./.github/actions/setup-rust"))
        .unwrap();
    assert_eq!(smoke_setup["if"].as_str(), Some("matrix.target == 'rust'"));
}

#[cfg(unix)]
#[test]
fn rust_setup_exports_the_selected_toolchain_for_generated_projects() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let action: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        &fs::read_to_string(".github/actions/setup-rust/action.yml").unwrap(),
    )
    .unwrap();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("rust-setup-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let rustup = root.join("rustup");
    // A different version catches hard-coded exports, and the trailing
    // explanation catches exporting the whole active-toolchain output.
    fs::write(
        &rustup,
        "#!/bin/sh\nif [ \"$*\" = 'show active-toolchain' ]; then\n  echo '1.97.3-x86_64-unknown-linux-gnu (overridden by rust-toolchain.toml)'\nelif [ \"$*\" != show ]; then\n  exit 99\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755)).unwrap();
    let env_file = root.join("github-env");
    let output = Command::new("bash")
        .args([
            "-e",
            "-o",
            "pipefail",
            "-c",
            action["runs"]["steps"][0]["run"].as_str().unwrap(),
        ])
        .env(
            "PATH",
            format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
        )
        .env("GITHUB_ENV", &env_file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(env_file).unwrap(),
        "RUSTUP_TOOLCHAIN=1.97.3-x86_64-unknown-linux-gnu\n"
    );
    fs::remove_dir_all(root).unwrap();
}

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
        Some(
            "${{ !cancelled() && contains(fromJSON(needs.plan.outputs.jobs), 'build-site') && needs.generate-fixture.result == 'success' && (needs.plan.outputs.site != 'true' || needs.build-wasm.result == 'success') }}"
        )
    );
    assert_eq!(jobs["smoke"]["needs"][0].as_str(), Some("build-site"));
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
            Some("needs.plan.outputs.publish == 'true'")
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
    assert_eq!(archives["if"].as_str(), Some("always()"));
    assert_eq!(archives["with"]["name"].as_str(), Some("browse-archives"));
    let pages = jobs["assemble-site"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Upload Pages artifact"))
        .unwrap();
    assert!(pages.get("if").is_none());
    // Publication waits for the report that describes the exact archive bytes.
    assert_eq!(
        jobs["assemble-site"]["if"].as_str(),
        Some(
            "${{ !cancelled() && needs.plan.outputs.publish == 'true' && needs.build-site.result == 'success' && needs.archive-results.result == 'success' }}"
        )
    );
    assert!(
        jobs.get("deploy").is_none(),
        "PR validation must not carry deployment privileges"
    );
}

#[test]
fn head_and_label_changes_replace_the_previous_pr_run_without_draft_churn() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let events = ci["on"]["pull_request"]["types"].as_sequence().unwrap();
    assert_eq!(
        events,
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>(
            "[opened, synchronize, reopened, labeled, unlabeled]"
        )
        .unwrap()
        .as_sequence()
        .unwrap()
    );
    assert_eq!(
        ci["concurrency"]["group"].as_str(),
        Some(
            "validation-${{ github.event_name }}-${{ github.event.pull_request.number || github.ref }}"
        )
    );
    assert_eq!(
        ci["concurrency"]["cancel-in-progress"].as_str(),
        Some("${{ github.event_name == 'pull_request' || github.event_name == 'push' }}")
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
        consumer["needs"][0].as_str(),
        Some("build-campfire-compare-spinel")
    );
    assert_eq!(consumer["needs"][1].as_str(), Some("plan"));
    assert_eq!(
        producer["outputs"]["artifact-id"].as_str(),
        Some("${{ steps.binary.outputs.artifact-id }}")
    );
    assert_eq!(
        consumer["if"].as_str(),
        Some(
            "${{ !cancelled() && contains(fromJSON(needs.plan.outputs.jobs), 'campfire-compare-spinel') && needs.build-campfire-compare-spinel.outputs.artifact-id != '' }}"
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
        (
            "",
            "No Campfire comparison binary was uploaded. The default, minor-gc and verify-gen comparisons are blocked, not passed; see the producer failure above.\n",
        ),
        (
            "12345",
            "Campfire comparison binary uploaded; default, minor-gc and verify-gen comparisons can run.\n",
        ),
    ] {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let summary =
            std::env::temp_dir().join(format!("campfire-ready-{}-{unique}.md", std::process::id()));
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

#[cfg(unix)]
#[test]
fn shared_debug_roundhouse_reaches_campfire_consumers_via_roundhouse_bin() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let producer = &workflow["jobs"]["build-roundhouse"];
    assert_eq!(
        producer["outputs"]["roundhouse-bin-artifact-id"].as_str(),
        Some("${{ steps.roundhouse-bin.outputs.artifact-id }}")
    );
    let upload = producer["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["id"].as_str() == Some("roundhouse-bin"))
        .expect("producer uploads the debug binary");
    assert_eq!(
        upload["with"]["name"].as_str(),
        Some("roundhouse-debug-bin")
    );

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "roundhouse-bin-helper-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let marker = root.join("invoked.txt");
    let fake = root.join("fake-roundhouse");
    // Staged producer stand-in: records argv and, when given -o, writes a
    // minimal emit tree so campfire-suite can finish its emit branch.
    fs::write(
        &fake,
        format!(
            r#"#!/bin/bash
set -euo pipefail
printf 'fake-roundhouse' >> "{marker}"
printf ' %q' "$@" >> "{marker}"
printf '\n' >> "{marker}"
echo "fake-roundhouse:$*" >&2
out=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" || "$prev" == "--output" ]]; then out="$arg"; fi
  prev="$arg"
done
if [[ -n "$out" ]]; then
  mkdir -p "$out"
  # Empty SPINEL_TESTS: suite parses the list and runs zero files.
  printf 'SPINEL_TESTS :=\n\n.PHONY: all\nall:\n' > "$out/Makefile"
  mkdir -p "$out/db" "$out/storage"
  : > "$out/db/seed.sql"
  printf 'require_relative "app/models"\n' > "$out/boot.rb"
  mkdir -p "$out/app"
  : > "$out/app/models.rb"
fi
echo fake-ok
"#,
            marker = marker.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&fake).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake, perms).unwrap();

    let repo = std::env::current_dir().unwrap();
    let helper = repo.join("scripts/lib/roundhouse-bin.sh");

    // Direct helper: ROUNDHOUSE_BIN is consumed; cargo is not.
    // Drive via a small script file (no bash -c interpolation).
    let probe = root.join("probe-helper.sh");
    let probe_app = root.join("probe-app");
    let probe_out = root.join("probe-out");
    fs::create_dir_all(&probe_app).unwrap();
    fs::write(
        &probe,
        "#!/bin/bash\nset -euo pipefail\n. \"$HELPER\"\nroundhouse_run --target ruby \"$PROBE_APP\" -o \"$PROBE_OUT\"\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&probe).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&probe, perms).unwrap();
    let _ = fs::remove_file(&marker);
    let output = Command::new(&probe)
        .env("HELPER", &helper)
        .env("REPO_ROOT", &repo)
        .env("ROUNDHOUSE_BIN", &fake)
        .env("ROUNDHOUSE_BIN_TRACE", "1")
        .env("PROBE_APP", &probe_app)
        .env("PROBE_OUT", &probe_out)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("fake-ok"),
        "stdout must come from the staged binary: {stdout}"
    );
    assert!(
        stderr.contains("roundhouse-bin: exec") && stderr.contains("fake-roundhouse"),
        "trace must name the staged binary: {stderr}"
    );
    assert!(
        !stderr.contains("cargo run"),
        "must not fall back to cargo: {stderr}"
    );
    let invoked = fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        invoked.contains("fake-roundhouse"),
        "helper must exec the staged binary: {invoked}"
    );

    // Missing ROUNDHOUSE_BIN path must fail closed, not cargo-run.
    let missing = root.join("missing-roundhouse");
    let fail_probe = root.join("probe-missing.sh");
    fs::write(
        &fail_probe,
        "#!/bin/bash\nset -euo pipefail\n. \"$HELPER\"\nroundhouse_run --version\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&fail_probe).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fail_probe, perms).unwrap();
    let failed = Command::new(&fail_probe)
        .env("HELPER", &helper)
        .env("REPO_ROOT", &repo)
        .env("ROUNDHOUSE_BIN", &missing)
        .output()
        .unwrap();
    assert!(!failed.status.success(), "missing binary must not succeed");
    let err = String::from_utf8_lossy(&failed.stderr);
    assert!(
        err.contains("ROUNDHOUSE_BIN is not an executable file"),
        "fail-closed message: {err}"
    );

    // End-to-end (#317): a Campfire consumer emit branch must call the
    // supplied executable. campfire-suite is what campfire-conformance runs;
    // bypassing roundhouse_run for cargo run would miss the marker file.
    let app = root.join("mini-app");
    fs::create_dir_all(&app).unwrap();
    let out = root.join("suite-out");
    let tally = root.join("tally.txt");
    let _ = fs::remove_file(&marker);
    let suite = Command::new(repo.join("scripts/campfire-suite"))
        .args([
            "--no-stubs",
            "--out",
            out.to_str().unwrap(),
            "--tally",
            tally.to_str().unwrap(),
            app.to_str().unwrap(),
        ])
        .env("ROUNDHOUSE_BIN", &fake)
        .env("ROUNDHOUSE_BIN_TRACE", "1")
        // Not the repo cwd: relative paths must still resolve via abs_path,
        // and the binary branch must not silently become cargo under REPO_ROOT.
        .current_dir(&root)
        .output()
        .unwrap();
    let suite_out = String::from_utf8_lossy(&suite.stdout);
    let suite_err = String::from_utf8_lossy(&suite.stderr);
    assert!(
        suite.status.success(),
        "campfire-suite emit branch failed: status={:?}\nstdout={suite_out}\nstderr={suite_err}",
        suite.status
    );
    let invoked = fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        invoked.contains("fake-roundhouse") && invoked.contains("--target"),
        "campfire-suite must exec ROUNDHOUSE_BIN on the emit branch: {invoked}\nstderr={suite_err}"
    );
    assert!(
        !suite_err.contains("cargo run") && !invoked.contains("cargo"),
        "campfire-suite must not rebuild via cargo: stderr={suite_err} invoked={invoked}"
    );

    let _ = fs::remove_dir_all(&root);
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
            (
                "bin/cargo",
                "#!/bin/sh\necho emit-output >&2\nexit \"$EMIT_EXIT\"\n",
            ),
            (
                "bin/make",
                "#!/bin/sh\necho build-output >&2\nif [ \"$RETAINED_C\" = true ]; then\n  echo \"spinel: the generated C is kept at $PWD/actual.c\" >&2\nfi\necho \"note: the generated C is kept at $PWD/stale.c\"\nexit \"$BUILD_EXIT\"\n",
            ),
            (
                "spinel-dist/spinel",
                "#!/bin/sh\n[ \"$1\" = --version ] || exit 99\necho compiler-version\n",
            ),
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
    assert_eq!(db["needs"][0].as_str(), Some("build-spinel"));
    assert_eq!(db["needs"][1].as_str(), Some("plan"));
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
    assert_eq!(
        gc["needs"][0].as_str(),
        Some("build-campfire-compare-spinel")
    );
    assert_eq!(gc["needs"][1].as_str(), Some("plan"));
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
    assert!(gc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .all(|step| step["run"].as_str() != Some(command)));
}

#[test]
fn pr_reuse_never_masks_validation_failures_or_changes_the_job_graph() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = ci["jobs"].as_mapping().unwrap();
    let mut enabled = Vec::new();
    for (name, job) in jobs {
        let name = name.as_str().unwrap();
        let steps = job["steps"].as_sequence().unwrap();
        let Some(probe) = steps
            .iter()
            .find(|step| step["id"].as_str() == Some("reuse"))
        else {
            continue;
        };
        enabled.push(name);
        assert_eq!(job["permissions"]["actions"].as_str(), Some("read"));
        assert!(
            probe["if"]
                .as_str()
                .unwrap()
                .contains("github.event_name == 'pull_request'"),
            "main must not even look up old receipts: {name}"
        );
        assert_eq!(probe["continue-on-error"].as_bool(), Some(true));
        assert!(probe["run"]
            .as_str()
            .unwrap()
            .contains("scripts/ci-reuse.py probe"));
        assert!(job.get("continue-on-error").is_none());
        let validation_ids: &[&str] = match name {
            "store-check" => {
                assert_eq!(job["needs"][0].as_str(), Some("generate-fixture"));
                assert_eq!(job["needs"][1].as_str(), Some("plan"));
                &["build", "check"]
            }
            "writebook-inventory" => {
                assert_eq!(job["needs"].as_str(), Some("plan"));
                &["inventory", "report"]
            }
            "browser-smoke-typescript" => {
                assert_eq!(job["needs"][0].as_str(), Some("generate-fixture"));
                assert_eq!(job["needs"][1].as_str(), Some("plan"));
                &["browser"]
            }
            "smoke" => {
                assert_eq!(job["needs"][0].as_str(), Some("build-site"));
                assert_eq!(
                    probe["if"].as_str(),
                    Some("github.event_name == 'pull_request' && matrix.target == 'rust'")
                );
                &["smoke"]
            }
            _ => panic!("unaudited reuse job: {name}"),
        };
        for &id in validation_ids {
            let step = steps
                .iter()
                .find(|step| step["id"].as_str() == Some(id))
                .unwrap();
            assert!(step.get("continue-on-error").is_none());
            let guard = step["if"].as_str().unwrap();
            assert!(guard
                .contains("steps.reuse.outcome != 'success' || steps.reuse.outputs.hit != 'true'"));
        }
        let receipt = steps
            .iter()
            .find(|step| step["id"].as_str() == Some("execution"))
            .unwrap();
        for &id in validation_ids {
            assert!(receipt["if"]
                .as_str()
                .unwrap()
                .contains(&format!("steps.{id}.outcome == 'success'")));
        }
        assert_eq!(receipt["continue-on-error"].as_bool(), Some(true));
    }
    enabled.sort();
    assert_eq!(
        enabled,
        [
            "browser-smoke-typescript",
            "smoke",
            "store-check",
            "writebook-inventory"
        ]
    );
    assert!(ci["on"].get("pull_request_target").is_none());
}

#[test]
fn downstream_reuse_keeps_producers_fresh_and_other_matrix_checks_running() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let worker = ci["jobs"]["browser-smoke-typescript"]["steps"]
        .as_sequence()
        .unwrap();
    let prepare = worker
        .iter()
        .position(|s| s["name"].as_str() == Some("Emit and build current SharedWorker project"))
        .unwrap();
    let probe = worker
        .iter()
        .position(|s| s["id"].as_str() == Some("reuse"))
        .unwrap();
    assert!(prepare < probe);
    assert!(worker[prepare].get("if").is_none());
    assert_eq!(worker[prepare]["run"].as_str(), Some("npm run prebuild"));
    let validate = worker
        .iter()
        .find(|s| s["id"].as_str() == Some("browser"))
        .unwrap();
    assert_eq!(validate["run"].as_str(), Some("npm run test-only"));
    let compare = &ci["jobs"]["compare"];
    assert_eq!(compare["permissions"]["actions"].as_str(), Some("read"));
    let steps = compare["steps"].as_sequence().unwrap();
    let inflector = steps
        .iter()
        .find(|s| s["id"].as_str() == Some("inflector"))
        .unwrap();
    assert!(inflector.get("continue-on-error").is_none());
    assert_eq!(
        inflector["env"]["ROUNDHOUSE_CI_REUSE"].as_str(),
        Some("${{ github.event_name == 'pull_request' && 'rust-inflector' || '' }}")
    );
    let upload = steps
        .iter()
        .find(|s| {
            s["with"]["name"].as_str()
                == Some("ci-executed-rust-inflector-${{ github.run_attempt }}")
        })
        .unwrap();
    assert!(upload["if"]
        .as_str()
        .unwrap()
        .contains("steps.inflector.outputs.recorded == 'true'"));
    assert_eq!(upload["continue-on-error"].as_bool(), Some(true));
    let dom = steps
        .iter()
        .find(|s| s["name"].as_str() == Some("scripts/compare ${{ matrix.target }}"))
        .unwrap();
    assert_eq!(dom["if"].as_str(), Some("${{ !cancelled() }}"));
    let smoke = ci["jobs"]["smoke"]["steps"].as_sequence().unwrap();
    let validate = smoke
        .iter()
        .find(|s| s["id"].as_str() == Some("smoke"))
        .unwrap();
    assert!(validate["if"]
        .as_str()
        .unwrap()
        .starts_with("matrix.target != 'rust' ||"));
    assert!(validate["run"]
        .as_str()
        .unwrap()
        .contains("--work-dir \"$RUNNER_TEMP/ci-smoke-validation\""));
}

#[cfg(unix)]
#[test]
fn pr_reuse_receipts_are_checked_against_adversarial_inputs() {
    let result = std::process::Command::new("python3")
        .args(["tests/ci_reuse_test.py", "-v"])
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // Exercise the actual generated README, not just a synthetic fixture.
        .env(
            "ROUNDHOUSE_TEST_RUST_README",
            roundhouse::project::target_readme(roundhouse::project::BuildTarget::Rust),
        )
        .output()
        .expect("CI reuse tests require python3 (available on hosted runners)");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn reused_checks_keep_cargo_dependencies_locked_and_upload_only_execution_receipts() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    for (name, expected_cargo_commands) in [("store-check", 1), ("writebook-inventory", 2)] {
        let steps = ci["jobs"][name]["steps"].as_sequence().unwrap();
        let commands: Vec<_> = steps
            .iter()
            .filter_map(|step| step["run"].as_str())
            .flat_map(str::lines)
            .map(str::trim)
            .filter(|line| line.starts_with("cargo "))
            .collect();
        assert_eq!(commands.len(), expected_cargo_commands);
        for command in commands {
            assert!(
                command.split_whitespace().any(|flag| flag == "--locked"),
                "{name}: unrecorded dependency resolution invalidates reuse: {command}"
            );
        }
        let uploads: Vec<_> = steps
            .iter()
            .filter(|step| {
                step["with"]["name"]
                    .as_str()
                    .is_some_and(|artifact| artifact.starts_with("ci-executed-"))
            })
            .collect();
        assert_eq!(uploads.len(), 1);
        assert_eq!(
            uploads[0]["if"].as_str(),
            Some("success() && steps.execution.outcome == 'success'")
        );
        assert_eq!(uploads[0]["continue-on-error"].as_bool(), Some(true));
        assert_eq!(uploads[0]["with"]["retention-days"].as_u64(), Some(7));
    }
}
