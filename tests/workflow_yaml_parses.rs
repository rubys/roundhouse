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
