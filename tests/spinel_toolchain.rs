//! Spinel toolchain integration test — compiles the emitted real-blog
//! tests via the spinel AOT compiler and runs the resulting native
//! binaries. Mirrors `ruby_toolchain.rs`: same emit, same 4 test
//! suites, swapped runner.
//!
//! Two differences from the Ruby toolchain test:
//!   1. `runtime/db.rb` is the FFI-backed shim (`runtime/spinel/db.rb`,
//!      module Db over libsqlite3) rather than the gem-backed sibling.
//!   2. The runner is the scaffold Makefile's `spinel-test` target,
//!      which compiles each `test/<dir>/<stem>.rb` via `$(SPINEL)` and
//!      executes the resulting binary. `$(SPINEL)` defaults to `spinel`
//!      on PATH — set the `SPINEL` env var to override.
//!
//! Marked `#[ignore]` — CI-only. Invoke:
//!
//!     cargo test --test spinel_toolchain -- --ignored --nocapture
//!
//! Prerequisites for local runs: `spinel` on PATH (or `SPINEL=...`),
//! and `libsqlite3.so` discoverable at link time (`libsqlite3-dev` on
//! Debian/Ubuntu; macOS ships it).
//!
//! Suites validated: same 4 as ruby_toolchain — article + comment
//! model tests, articles + comments controller tests. Wider coverage
//! (article_broadcasts, views suite) tracked in
//! `project_lowered_ir_gaps_for_runnability`.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;

use roundhouse::ingest::ingest_app;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/class_attribute.rs"]
mod class_attribute;
#[path = "support/class_configuration.rs"]
mod class_configuration;
#[path = "support/rails_root_join.rs"]
mod rails_root_join;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn finite_concern_class_configuration_runs_natively() {
    for (overlay, assertions) in [
        (class_configuration::overlay(), class_configuration::ASSERTIONS),
        (class_configuration::empty_overlay(), class_configuration::EMPTY_ASSERTIONS),
    ] {
        let run = overlay.run_spinel(assertions);
        run.assert_passes();
        assert!(run.stdout.contains("finite class configuration contract passed"));
    }
}

/// The native half of `emit_and_run::concern_class_attribute_macros_run_at_class_load`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn concern_class_attribute_macros_run_at_class_load_natively() {
    let run = class_attribute::overlay().run_spinel(class_attribute::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute contract passed"));
}

/// The native half of `emit_and_run::concern_class_attribute_set_to_nil_is_not_unset`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn concern_class_attribute_set_to_nil_is_not_unset_natively() {
    let run = class_attribute::nil_overlay().run_spinel(class_attribute::NIL_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute nil contract passed"));
}

/// The native half of `emit_and_run::rails_root_join_takes_any_number_of_parts`:
/// the fixtures never call `join` with more than one part, so no other
/// Spinel lane compiles the variadic `Rails::AppPath#join`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn rails_root_join_takes_any_number_of_parts_natively() {
    let run = rails_root_join::overlay().run_spinel(rails_root_join::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root.join contract passed"));
}

/// The native half of `rails_health_check::the_rails_health_check_answers_up`:
/// `/up` routes to the synthesized `Rails::HealthController`, which
/// compiles and answers the green page. `main.rb` boots the server
/// under AOT, so its `instantiate_controller` arm is checked as text.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn the_rails_health_check_answers_up_natively() {
    let run = emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\n",
        )
        .run_spinel(r#"
routed = false
RouteTable.table.each do |route|
  routed = true if route.verb == "GET" && route.pattern == "/up" && route.controller == :rails_health && route.action == :show
end
raise "no GET /up route to rails_health#show" unless routed
c = Rails::HealthController.new
c.process_action(:show)
raise "status #{c.status}" unless c.status == 200
raise "body #{c.body}" unless c.body == "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>"
puts "rails health contract passed"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("rails health contract passed"));
    let main = std::fs::read_to_string(run.emitted.join("main.rb")).expect("main.rb");
    assert!(main.contains("when :rails_health then Rails::HealthController.new"), "{main}");
}

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-spinel-{tag}"))
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).expect("mkdir");
        for entry in std::fs::read_dir(src).expect("readdir") {
            let entry = entry.expect("entry");
            copy_tree(&entry.path(), &dst.join(entry.file_name()));
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::copy(src, dst).expect("copy file");
    }
}

/// Build the scratch project: the REAL spinel base file set, exactly as
/// `project::spinel_files` assembles it before `spin_shape`.
///
/// This used to hand-copy an enumerated list of runtime files on top of
/// the scaffold. Its own comment called that "a FOURTH registration
/// point for a new runtime file" and predicted the failure mode — "a
/// miss shows up as `cannot load such file` from spinel rather than
/// from anything the unit tests reach" — which is exactly how it broke
/// once `test/test_helper.rb` started requiring `main.rb` (whose chain
/// is complete). Deleted in favour of the set that ships.
fn generate_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    let mut app = ingest_app(fixture).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, fixture).expect("spinel base files");
    roundhouse::project::write_to_dir(&files, scratch).expect("write spinel tree");

    // The framework runtime's OWN tests (broadcasts/cgi_io + the
    // integration/views/models/tools subdirs) are a harness concern, not
    // something an app archive ships — overlay them so this job keeps
    // covering them alongside the app's emitted suite.
    copy_tree(Path::new("runtime/spinel/test"), &scratch.join("test"));

    // …but not that tree's `test_helper.rb`: the shipped tree already
    // carries the per-app rendered one (`render_test_helper`), and the
    // source copy is the blog-shaped stand-in it exists to replace.
    for (path, content) in &files {
        if path == "test/test_helper.rb" {
            std::fs::write(scratch.join("test/test_helper.rb"), content)
                .expect("restore rendered test_helper");
        }
    }
}

#[test]
#[ignore]
fn real_blog_spinel_tests_pass() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("make")
        .arg("spinel-test")
        .current_dir(&scratch)
        .output()
        .expect("spawn make spinel-test");

    assert!(
        output.status.success(),
        "make spinel-test failed\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Exercise the shipped CLI/spin package, not the pre-spin_shape test overlay.
#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite/jemalloc development libraries"]
fn identical_cli_emission_reuses_native_build_but_changed_ruby_rebuilds() {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn snapshot(root: &Path, dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        let mut files = BTreeMap::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(snapshot(root, &path));
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    (
                        fs::read(&path).unwrap(),
                        fs::metadata(&path).unwrap().modified().unwrap(),
                    ),
                );
            }
        }
        files
    }

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = scratch_dir(&format!("repeat-{}-{unique}", std::process::id()));
    let fixture = scratch.join("source");
    let dest = scratch.join("output");
    copy_tree(roundhouse::fixtures::real_blog(), &fixture);
    fs::write(fixture.join("public/freshness.bin"), [0xff, 0x00, 0x80]).unwrap();
    let emit = || {
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", "spinel"])
            .arg(&fixture)
            .arg("-o")
            .arg(&dest)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "strict emit failed: {stderr}");
        stderr
    };
    let build = || {
        let output = Command::new("spin")
            .args(["build", "blog"])
            .current_dir(&dest)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "spin build failed:\n{stdout}\n{stderr}"
        );
        eprintln!("{stdout}{stderr}");
        (stdout, stderr)
    };

    let first_emit = emit();
    let generated = snapshot(&dest, &dest);
    assert!(generated.contains_key(Path::new("spin.toml")));
    assert!(
        generated
            .keys()
            .any(|p| p.extension().is_some_and(|e| e == "rbs"))
    );
    assert_eq!(
        generated[Path::new("public/freshness.bin")].0,
        [0xff, 0x00, 0x80]
    );
    assert!(
        first_emit.contains(&format!("emitted {} files", generated.len())),
        "{first_emit}"
    );
    let (stdout, stderr) = build();
    assert!(stdout.contains("build blog\n"), "{stdout}");
    assert!(
        stderr.contains("bin/blog.rb -> "),
        "compiler invocation missing: {stderr}"
    );
    let executable = dest.join("build/bin/blog");
    let built_at = fs::metadata(&executable).unwrap().modified().unwrap();

    // Spinel compares integer seconds with a strict >. Waiting makes the
    // old unconditional writer reliably invalidate the already built binary.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(emit(), first_emit);
    let repeated = snapshot(&dest, &dest);
    for (path, expected) in &generated {
        assert_eq!(&repeated[path], expected, "{}", path.display());
    }
    let (stdout, stderr) = build();
    assert!(stdout.contains("build blog (up to date)"), "{stdout}");
    assert!(
        !stderr.contains("bin/blog.rb -> "),
        "unexpected compiler invocation: {stderr}"
    );
    assert_eq!(
        fs::metadata(&executable).unwrap().modified().unwrap(),
        built_at
    );

    std::thread::sleep(Duration::from_secs(2));
    let source = fixture.join("app/controllers/articles_controller.rb");
    let before = fs::read_to_string(&source).unwrap();
    let after = before.replace(
        "Article was successfully created.",
        "Article was successfully changed.",
    );
    assert_ne!(
        before, after,
        "fixture no longer contains the mutation literal"
    );
    fs::write(source, after).unwrap();
    emit();
    let ruby = Path::new("app/controllers/articles_controller.rb");
    let changed = fs::read(dest.join(ruby)).unwrap();
    assert_ne!(
        changed, generated[ruby].0,
        "source change must reach generated Ruby"
    );
    assert!(String::from_utf8_lossy(&changed).contains("Article was successfully changed."));
    assert_eq!(
        fs::metadata(dest.join("spin.toml"))
            .unwrap()
            .modified()
            .unwrap(),
        generated[Path::new("spin.toml")].1
    );
    let (stdout, stderr) = build();
    assert!(stdout.contains("build blog\n"), "{stdout}");
    assert!(
        stderr.contains("bin/blog.rb -> "),
        "changed Ruby did not invoke compiler: {stderr}"
    );
    assert!(fs::metadata(&executable).unwrap().modified().unwrap() > built_at);
    fs::remove_dir_all(scratch).unwrap();
}
