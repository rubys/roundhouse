//! Framework-test transpile gate (spinel AOT target).
//!
//! Mirrors `framework_tests_ruby.rs` but compiles the emitted test
//! through `spinel --rbs sig` and runs the resulting native binary
//! instead of stock CRuby. Catches spinel-specific divergences in the
//! framework-runtime layer (type-narrowing gaps, RBS application,
//! monomorphization edge cases) that the source-side `framework_tests_
//! ruby` gate can't surface because CRuby is dynamic.
//!
//! Marked `#[ignore]` (CI-only). Invoke:
//!
//!     PATH=$HOME/git/spinel:$PATH cargo test --test framework_tests_spinel -- --ignored --nocapture
//!
//! Status: CI job is `continue-on-error: true` while spinel-side
//! gaps close. The `view_helpers` false-positive previously listed
//! here (Article+ViewHelpersTest dual-class shape silently dropped
//! the test class) is closed by issue #4 — `ingest_test_file` now
//! picks the `*Test` class regardless of source order and routes
//! top-level helpers through `inner_classes`. The N≥1 autorun-shim
//! check below catches any future regression of the same class.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

fn scratch_dir(tag: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-framework-tests-spinel").join(tag)
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

/// Move every `<scratch>/{runtime,test}/**/*.rbs` to
/// `<scratch>/sig/{runtime,test}/<rel>.rbs`. Same pattern as
/// `spinel_toolchain.rs::reroute_runtime_rbs_to_sig`.
fn reroute_rbs_to_sig(scratch: &Path) {
    fn walk(dir: &Path, src_root: &Path, sig_root: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, src_root, sig_root);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rbs") {
                let rel = path.strip_prefix(src_root).expect("under src root");
                let dst = sig_root.join(rel);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).expect("mkdir sig parent");
                }
                std::fs::rename(&path, &dst).expect("mv .rbs to sig/");
            }
        }
    }
    let runtime_dir = scratch.join("runtime");
    let sig_runtime = scratch.join("sig").join("runtime");
    walk(&runtime_dir, &runtime_dir, &sig_runtime);

    let test_dir = scratch.join("test");
    let sig_test = scratch.join("sig").join("test");
    walk(&test_dir, &test_dir, &sig_test);
}

fn build_and_run(test_file: &Path, tag: &str) {
    let scratch = scratch_dir(tag);
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("create scratch");

    let runtime_ruby = Path::new("runtime/ruby");
    let scratch_runtime = scratch.join("runtime");
    std::fs::create_dir_all(&scratch_runtime).expect("mkdir runtime");
    // EVERYTHING under `runtime/ruby` except its own `test/` dir — this
    // was two hand-maintained lists (four subdirs, eight files) and it
    // had drifted: `rails.rb` was absent, so `cookies.signed`, which
    // keys off `Rails.application.secret_key_base`, compiled to a call
    // on `unknown`. The framework runtime is a require graph, not a
    // curated subset. Subdirectories carry their own `.rbs` sidecars;
    // a top-level stem's sidecar belongs at `sig/runtime/` (below).
    let mut stems: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(runtime_ruby).expect("readdir runtime/ruby") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "test" {
            continue;
        }
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &scratch_runtime.join(&name));
        } else if name.ends_with(".rb") {
            stems.push(name);
        }
    }
    stems.sort();
    for entry in stems.iter().map(|s| s.as_str()) {
        let src = runtime_ruby.join(entry);
        if src.exists() {
            std::fs::copy(&src, scratch_runtime.join(entry))
                .unwrap_or_else(|_| panic!("copy {entry}"));
        }
        // …and its `.rbs` sidecar, which the subdir walk above already
        // carries for `active_record/` &c but which this flat loop
        // dropped. Production does copy it (`project.rs::spinel_files`
        // pairs every stem with `sig/runtime/<stem>.rbs`), so a runtime
        // file whose typing depends on the sidecar compiled here
        // against inference alone — a strictly harder problem than the
        // one the real emit poses, and one `action_text.rb` lost:
        // `parse_attributes`'s declared `Hash[String, String]` return
        // widened, and the `attrs[name] = …` in its caller compiled as
        // a STRING index-assign that raised IndexError at run time.
        let rbs = runtime_ruby.join(entry.replace(".rb", ".rbs"));
        if rbs.exists() {
            let sig_runtime = scratch.join("sig/runtime");
            std::fs::create_dir_all(&sig_runtime).expect("mkdir sig/runtime");
            std::fs::copy(&rbs, sig_runtime.join(entry.replace(".rb", ".rbs")))
                .unwrap_or_else(|_| panic!("copy sidecar for {entry}"));
        }
    }

    // Spinel-specific shims that the framework runtime calls into but
    // doesn't itself define: Base64 (used by ActionView::ViewHelpers
    // .turbo_stream_from), JSON (used by the same). Same pattern the
    // real-blog scaffold uses.
    let runtime_spinel = Path::new("runtime/spinel");
    for entry in ["base64.rb", "json.rb", "message_digest.rb"] {
        let src = runtime_spinel.join(entry);
        if src.exists() {
            std::fs::copy(&src, scratch_runtime.join(entry))
                .unwrap_or_else(|_| panic!("copy {entry}"));
        }
    }

    std::fs::create_dir_all(scratch.join("test")).expect("mkdir test");
    // Minimal spinel-compatible test_helper. Provides TestBase and
    // pulls in the framework runtime modules via require_relative.
    // ar_base_test is disabled in this runner (the prior
    // `FrameworkTestAdapter` mock didn't survive spinel
    // monomorphization; a follow-on session will re-enable it wired
    // against `runtime/spinel/{db,sqlite_adapter}.rb` + RBS).
    let helper = r#"# Auto-generated by framework_tests_spinel.rs.
require_relative "../runtime/base64"
require_relative "../runtime/json_impl"
require_relative "../runtime/inflector"
require_relative "../runtime/active_record"
require_relative "../runtime/action_view/view_helpers"
require_relative "../runtime/action_view/view_helpers_ext"
require_relative "../runtime/action_dispatch/router"
require_relative "../runtime/action_controller/base"
require_relative "../runtime/message_digest"
require_relative "../runtime/rails"
require_relative "../runtime/action_controller/message_verifier"
require_relative "../runtime/action_controller/cookies"

class TestBase
  def initialize
  end

  def setup
  end

  def teardown
  end

  def assert_match(pattern, value, msg = nil)
    raise(msg || "assert_match: nil value") if value.nil?
    return if value =~ pattern
    raise(msg || "assert_match failed")
  end
end
"#;
    std::fs::write(scratch.join("test/test_helper.rb"), helper)
        .expect("write minimal test_helper");

    let source = std::fs::read(test_file)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_file.display()));
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest framework test file")
        .expect("framework test file should contain a test class");

    let mut app = App::new();
    app.test_modules.push(test_module);
    Analyzer::new(&app).analyze(&mut app);

    for file in ruby::emit_spinel(&app) {
        if file.path == PathBuf::from("test/test_helper.rb") {
            continue;
        }
        let path = scratch.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir emit parent");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }

    // After the emit, reroute .rbs sidecars into sig/. spinel's
    // `--rbs sig` flag walks sig/, not the file-adjacent layout.
    reroute_rbs_to_sig(&scratch);

    // Locate the emitted test file (same logic as the ruby variant).
    let test_dir = scratch.join("test/models");
    let emitted_test = std::fs::read_dir(&test_dir)
        .unwrap_or_else(|e| panic!("readdir {}: {e}", test_dir.display()))
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().is_some_and(|x| x == "rb"))
        .expect("find emitted test file");
    let test_rel = emitted_test
        .path()
        .strip_prefix(&scratch)
        .expect("emitted path under scratch")
        .to_string_lossy()
        .into_owned();
    let stem = emitted_test
        .path()
        .file_stem()
        .expect("file stem")
        .to_string_lossy()
        .into_owned();
    let bin_path = format!("build/{stem}");

    std::fs::create_dir_all(scratch.join("build")).expect("mkdir build");

    // Compile with spinel.
    let compile = Command::new("spinel")
        .arg("--rbs")
        .arg("sig")
        .arg(&test_rel)
        .arg("-o")
        .arg(&bin_path)
        .current_dir(&scratch)
        .output()
        .expect("spawn spinel");

    assert!(
        compile.status.success(),
        "spinel compile failed: {} (emitted to {})\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        emitted_test.path().display(),
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    // Execute the resulting binary.
    let output = Command::new(format!("./{bin_path}"))
        .current_dir(&scratch)
        .output()
        .expect("spawn test binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "framework test failed: {} (binary {})\n\
         === stdout ===\n{}\n\
         === stderr ===\n{}",
        test_file.display(),
        bin_path,
        stdout,
        stderr,
    );

    assert_tests_ran(&stdout, test_file, &emitted_test.path());
}

/// Defense against issue #4: the spinel autorun shim prints
/// `<ClassName>: <N> tests passed` after running every `test_*`
/// method. When emit-routing drops the `*Test` class (e.g. when the
/// source file also defines a `< AR::Base` helper class that gets
/// picked up first), N is 0 and the binary still exits clean. Parse
/// the shim's summary line and require at least one test ran.
fn assert_tests_ran(stdout: &str, test_file: &Path, emitted: &Path) {
    let count = stdout.lines().find_map(parse_spinel_tests_passed).unwrap_or_else(|| {
        panic!(
            "framework test for {} produced no spinel autorun summary \
             line — cannot verify tests actually ran (see issue #4).\n\
             emitted: {}\n=== stdout ===\n{}",
            test_file.display(),
            emitted.display(),
            stdout,
        )
    });
    assert!(
        count >= 1,
        "framework test for {} reported 0 tests passed — \
         emit-routing likely dropped the test class (see issue #4).\n\
         emitted: {}\n=== stdout ===\n{}",
        test_file.display(),
        emitted.display(),
        stdout,
    );
}

/// Match `<ClassName>: <N> tests passed`. The shim emits this exactly,
/// so look for the suffix and parse the digits immediately before.
fn parse_spinel_tests_passed(line: &str) -> Option<usize> {
    let line = line.trim();
    let idx = line.find(" tests passed")?;
    line[..idx].split_whitespace().last()?.parse::<usize>().ok()
}

// ar_base_test_passes_under_spinel — disabled. base_test.rb depends
// on FrameworkTestAdapter (now removed). Follow-on session will rewrite
// the test to wire each target against its real sqlite adapter and
// re-add this runner.

#[test]
#[ignore]
fn errors_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/active_record/errors_test.rb"),
        "errors",
    );
}

#[test]
#[ignore]
fn ac_base_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/base_test.rb"),
        "ac_base",
    );
}

/// CookieJar (`cookies[:k]`, and the `each` walk lobsters'
/// `remove_unknown_cookies` needs). Ruby-family lanes only — the jar is a
/// reopen outside the strict-target runtime tables, so unlike `ac_base`
/// this has no crystal/kotlin/swift/typescript sibling.
#[test]
#[ignore]
fn ac_cookies_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_controller/cookies_test.rb"),
        "ac_cookies",
    );
}

#[test]
#[ignore]
fn router_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_dispatch/router_test.rb"),
        "router",
    );
}

#[test]
#[ignore]
fn view_helpers_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/view_helpers_test.rb"),
        "view_helpers",
    );
}

// Ruby-family only (see the test file's header). This is the half that
// matters most: the date helpers moved off the CRuby overlay so the
// spinel tree could carry them, and this is what proves both runtimes
// render the same wording.
#[test]
#[ignore]
fn date_helper_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_view/date_helper_test.rb"),
        "date_helper",
    );
}

#[test]
#[ignore]
fn inflector_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/inflector_test.rb"),
        "inflector",
    );
}

/// `ActionText::Content` under spinel — the same measured-against-Rails
/// cases the CRuby sibling runs. This is the lane that prices the
/// scanner's shapes (two-arg slice, parallel single-type arrays, no
/// regex) against the strict whole-graph check.
#[test]
#[ignore]
fn action_text_test_passes_under_spinel() {
    build_and_run(
        Path::new("runtime/ruby/test/action_text_test.rb"),
        "action_text",
    );
}
