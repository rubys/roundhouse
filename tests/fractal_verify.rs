//! `fractal verify`: equal fingerprints verify an overlay; a changed method,
//! a rewrite no method sits on, or a relative constant compiled in a new
//! scope does not. Needs `ruby` on the path; without it every test that
//! boots Ruby returns early.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::fractal::MANIFEST;
use roundhouse::emit::fractal::verify::{Options, Outcome, verify};

const MODEL: &str = "lib/order.rb";

/// What Active Support's `delegate` would have done at boot: compile a
/// method from a string, with the call's file and line.
const EVALUATED: &str = "class Order\n  module_eval(\"def name = ::Kernel.format('%s', shop)\", __FILE__, __LINE__)\n\n  attr_reader :shop\nend\n";

/// The same method written where the call was, on the same line.
const WRITTEN: &str = "class Order\n  def name = ::Kernel.format('%s', shop)\n\n  attr_reader :shop\nend\n";

fn ruby_available() -> bool {
    let available = Command::new("ruby").args(["-e", "exit 0"]).output().is_ok_and(|output| output.status.success());
    if !available {
        eprintln!("ruby is not available; skipping");
    }
    available
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// An app whose `lib/order.rb` is `original`, an overlay replacing it
    /// with `rewritten`, and a report naming a rewrite at each of `rewrites`
    /// (`path:line:col`).
    fn new(name: &str, original: &str, rewritten: &str, rewrites: &[&str]) -> Self {
        Fixture::with_pass(name, original, rewritten, rewrites, "test-pass")
    }

    /// As [`Fixture::new`], with the rewrites made by `pass`.
    fn with_pass(name: &str, original: &str, rewritten: &str, rewrites: &[&str], pass: &str) -> Self {
        let root = std::env::temp_dir().join(format!("roundhouse-fractal-verify-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        put(&root.join("app"), MODEL, original);
        put(&root.join("overlay"), MODEL, rewritten);
        put(&root.join("overlay"), MANIFEST, &format!(r#"{{"compiled": ["{MODEL}"], "passes": ["{pass}"], "rewritten": ["{MODEL}"]}}"#));
        let rewrites: Vec<String> = rewrites.iter().map(|at| format!(r#"{{"at": "{at}", "pass": "{pass}", "from": "", "to": ""}}"#)).collect();
        put(&root, "report.json", &format!(r#"{{"compiled": ["{MODEL}"], "rewritten": ["{MODEL}"], "rewrites": [{}]}}"#, rewrites.join(", ")));
        Fixture { root }
    }

    fn app(&self) -> PathBuf {
        self.root.join("app")
    }

    fn options(&self) -> Options {
        Options {
            overlay: self.root.join("overlay"),
            app: self.app(),
            report: self.root.join("report.json"),
            ruby: vec!["ruby".to_string()],
            requires: Vec::new(),
            all: false,
            keep: None,
        }
    }

    fn verify(&self) -> Outcome {
        verify(&self.options()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn put(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn a_string_compiled_method_and_the_same_def_written_on_its_line_verify() {
    if !ruby_available() {
        return;
    }
    let fixture = Fixture::new("equal", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    let outcome = fixture.verify();
    assert!(outcome.verified(), "{}", outcome.render());
    assert_eq!(outcome.observed, ["lib/order.rb:2:3"]);
    assert_eq!(fs::read_to_string(fixture.app().join(MODEL)).unwrap(), EVALUATED, "the app's file is restored");
}

#[test]
fn every_module_fingerprints_the_same_in_two_processes() {
    if !ruby_available() {
        return;
    }
    // `--all` takes in Ruby's own builtins, whose bytecode holds C function
    // pointers, and modules named inside anonymous ones: both change with
    // every process and must not read as differences.
    let fixture = Fixture::new("all", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    let outcome = verify(&Options { all: true, ..fixture.options() }).unwrap();
    assert!(outcome.verified(), "{}", outcome.render());
    assert!(outcome.modules > 100, "{}", outcome.render());
}

#[test]
fn a_changed_body_is_a_bytecode_difference_attributed_to_its_rewrite() {
    if !ruby_available() {
        return;
    }
    let changed = WRITTEN.replace("'%s'", "'%s!'");
    let outcome = Fixture::new("body", EVALUATED, &changed, &["lib/order.rb:2:3"]).verify();
    assert!(!outcome.verified());
    assert_eq!(outcome.differences.len(), 1, "{:?}", outcome.differences);
    let difference = &outcome.differences[0];
    assert!(difference.starts_with("Order#name: bytecode differs (rewritten at lib/order.rb:2:3 [test-pass])"), "{difference}");
    assert!(difference.contains("%s!"), "{difference}");
}

#[test]
fn a_changed_visibility_is_a_difference() {
    if !ruby_available() {
        return;
    }
    let private = WRITTEN.replace("  def name", "  private def name");
    let outcome = Fixture::new("visibility", EVALUATED, &private, &["lib/order.rb:2:3"]).verify();
    assert!(outcome.differences.iter().any(|difference| difference.starts_with("Order#name: visibility public -> private")), "{:?}", outcome.differences);
}

#[test]
fn a_removed_method_is_a_difference() {
    if !ruby_available() {
        return;
    }
    let removed = WRITTEN.replace("  def name = ::Kernel.format('%s', shop)", "");
    let outcome = Fixture::new("removed", EVALUATED, &removed, &["lib/order.rb:2:3"]).verify();
    assert!(outcome.differences.iter().any(|difference| difference.starts_with("Order#name: missing after the rewrite")), "{:?}", outcome.differences);
}

#[test]
fn a_rewrite_no_method_sits_on_is_unobserved_not_verified() {
    if !ruby_available() {
        return;
    }
    let outcome = Fixture::new("unobserved", EVALUATED, WRITTEN, &["lib/order.rb:2:3", "lib/order.rb:3:1"]).verify();
    assert!(!outcome.verified());
    assert!(outcome.differences.is_empty(), "{:?}", outcome.differences);
    assert_eq!(outcome.observed, ["lib/order.rb:2:3"]);
    assert_eq!(outcome.unobserved.len(), 1);
    assert!(outcome.unobserved[0].starts_with("lib/order.rb:3:1 [test-pass]: no method"), "{}", outcome.unobserved[0]);
}

#[test]
fn a_relative_constant_compiled_in_a_new_scope_is_unverified() {
    if !ruby_available() {
        return;
    }
    // `Kernel` without `::`: equal bytecode, but a string compiled by
    // `module_eval` and a `def` in the class body look it up from
    // different lexical scopes.
    let evaluated = EVALUATED.replace("::Kernel", "Kernel");
    let written = WRITTEN.replace("::Kernel", "Kernel");
    let outcome = Fixture::new("relative", &evaluated, &written, &["lib/order.rb:2:3"]).verify();
    assert!(outcome.differences.is_empty(), "{:?}", outcome.differences);
    assert_eq!(outcome.unverified.len(), 1, "{}", outcome.render());
    assert!(outcome.unverified[0].contains("looks up Kernel by a relative path"), "{}", outcome.unverified[0]);
}

#[test]
fn the_app_is_restored_when_the_rewritten_app_fails_to_boot() {
    if !ruby_available() {
        return;
    }
    let broken = WRITTEN.replace("  attr_reader :shop", "  raise 'broken overlay'");
    let fixture = Fixture::new("broken", EVALUATED, &broken, &["lib/order.rb:2:3"]);
    let error = verify(&fixture.options()).expect_err("a boot that raises fails verification");
    assert!(error.contains("broken overlay"), "{error}");
    assert_eq!(fs::read_to_string(fixture.app().join(MODEL)).unwrap(), EVALUATED);
}

#[test]
fn a_report_from_another_build_is_refused_before_booting() {
    let fixture = Fixture::new("mismatch", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    put(&fixture.root.join("overlay"), MANIFEST, r#"{"compiled": [], "passes": [], "rewritten": ["lib/other.rb"]}"#);
    let error = verify(&fixture.options()).expect_err("mismatched report");
    assert!(error.contains("they are not from the same build"), "{error}");
}

#[test]
fn an_overlay_that_is_not_exactly_the_reported_files_is_refused_before_booting() {
    let fixture = Fixture::new("extra", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    put(&fixture.root.join("overlay"), "lib/stray.rb", "class Stray; end\n");
    let error = verify(&fixture.options()).expect_err("an extra overlay file");
    assert!(error.contains("also holds [\"lib/stray.rb\"]"), "{error}");
    assert!(!fixture.app().join("lib/stray.rb").exists());

    let fixture = Fixture::new("missing", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    fs::remove_file(fixture.root.join("overlay").join(MODEL)).unwrap();
    let error = verify(&fixture.options()).expect_err("a missing overlay file");
    assert!(error.contains("lacks [\"lib/order.rb\"]"), "{error}");

    let fixture = Fixture::new("keep-inside", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    let keep = fixture.root.join("overlay/prints");
    let error = verify(&Options { keep: Some(keep.clone()), ..fixture.options() }).expect_err("--keep inside the overlay");
    assert!(error.contains("is inside the overlay"), "{error}");
    assert!(!keep.exists());
}

#[test]
fn the_cli_exits_zero_only_when_verified() {
    if !ruby_available() {
        return;
    }
    let run = |fixture: &Fixture| {
        Command::new(env!("CARGO_BIN_EXE_fractal"))
            .arg("verify")
            .arg("--report")
            .arg(fixture.root.join("report.json"))
            .arg(fixture.root.join("overlay"))
            .arg(fixture.app())
            .output()
            .unwrap()
    };
    let verified = run(&Fixture::new("cli-equal", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]));
    assert!(verified.status.success(), "{}", String::from_utf8_lossy(&verified.stderr));
    assert!(String::from_utf8_lossy(&verified.stderr).contains("fractal: verified"));
    let changed = run(&Fixture::new("cli-changed", EVALUATED, &WRITTEN.replace("'%s'", "'%s!'"), &["lib/order.rb:2:3"]));
    assert_eq!(changed.status.code(), Some(1), "{}", String::from_utf8_lossy(&changed.stderr));
    assert!(String::from_utf8_lossy(&changed.stderr).contains("NOT verified"));
}

#[test]
fn a_relative_keep_directory_works_when_the_app_is_not_the_current_directory() {
    if !ruby_available() {
        return;
    }
    let fixture = Fixture::new("relative-keep", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    let output = Command::new(env!("CARGO_BIN_EXE_fractal"))
        .current_dir(&fixture.root)
        .args(["verify", "--report", "report.json", "--keep", "prints", "overlay", "app"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(fixture.root.join("prints/original.json").is_file() && fixture.root.join("prints/rewritten.json").is_file());
    assert_eq!(fs::read_to_string(fixture.root.join("prints/originals").join(MODEL)).unwrap(), EVALUATED);
}

#[cfg(unix)]
#[test]
fn an_interrupt_during_the_rewritten_boot_still_restores_the_app() {
    if !ruby_available() {
        return;
    }
    // Stands in for Ruby: during the rewritten boot (the overlay's `def` is
    // on disk) it sends SIGINT to verify, its parent, then fails.
    let fixture = Fixture::new("interrupt", EVALUATED, WRITTEN, &["lib/order.rb:2:3"]);
    let wrapper = fixture.root.join("interrupting-ruby");
    fs::write(&wrapper, "#!/bin/sh\nif grep -q '^  def name' lib/order.rb; then kill -INT $PPID; sleep 1; exit 1; fi\nexec ruby \"$@\"\n").unwrap();
    fs::set_permissions(&wrapper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fractal"))
        .arg("verify")
        .arg("--report")
        .arg(fixture.root.join("report.json"))
        .arg("--ruby")
        .arg(&wrapper)
        .arg(fixture.root.join("overlay"))
        .arg(fixture.app())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "verify survives the interrupt and fails: {output:?}");
    assert_eq!(fs::read_to_string(fixture.app().join(MODEL)).unwrap(), EVALUATED);
}

/// A method with a Duration chain on line 4, before `lower-duration`
/// rewrites it, and a branch after it. Loading it needs no Active Support:
/// nothing calls it.
const CHAIN: &str = "class Order\n  def recent\n    window = 1\n    list = [window, 5.days.ago, [1].map { 2.hours.ago }]\n    window > 0 ? list : []\n  end\nend\n";

/// The same method after the rewrite: different bytecode on line 4 only,
/// although the rewrite adds branches (and so labels) before line 5's.
const LOWERED: &str = "class Order\n  def recent\n    window = 1\n    list = [window, (::Time.zone ? ::Time.zone.now.plus_without_duration(-432000) : ::Time.now.advance(days: -5)), [1].map { (::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -7200, ::Time.zone) : ::Time.now + -7200) }]\n    window > 0 ? list : []\n  end\nend\n";

#[test]
fn a_lowering_pass_may_change_the_bytecode_of_its_rewritten_lines_only() {
    if !ruby_available() {
        return;
    }
    let rewrites = ["lib/order.rb:4:21", "lib/order.rb:4:43"];
    let outcome = Fixture::with_pass("lowered", CHAIN, LOWERED, &rewrites, "lower-duration").verify();
    assert!(outcome.verified(), "{}", outcome.render());
    assert_eq!(outcome.observed, rewrites);

    // A pass that keeps bytecode gets no such allowance.
    let outcome = Fixture::new("kept", CHAIN, LOWERED, &rewrites).verify();
    assert!(outcome.differences.iter().any(|difference| difference.starts_with("Order#recent: bytecode differs: ")), "{:?}", outcome.differences);

    // Nor does a line the lowering pass did not rewrite.
    let elsewhere = LOWERED.replace("window = 1", "window = 2");
    let outcome = Fixture::with_pass("lowered-elsewhere", CHAIN, &elsewhere, &rewrites, "lower-duration").verify();
    assert!(!outcome.verified());
    assert_eq!(outcome.differences.len(), 1, "{:?}", outcome.differences);
    assert!(outcome.differences[0].starts_with("Order#recent: bytecode differs on line 3, which no rewrite that changes bytecode is on"), "{}", outcome.differences[0]);
}

#[test]
fn a_lowered_default_argument_verifies_although_its_labels_move() {
    if !ruby_available() {
        return;
    }
    // The labels an optional parameter's default jumps to are bytecode
    // offsets, which move when the default's line compiles to more code.
    let original = "class Order\n  def recent(since = 5.days.ago, mode = 1)\n    [since, mode]\n  end\nend\n";
    let lowered = "class Order\n  def recent(since = (::Time.zone ? ::Time.zone.now.plus_without_duration(-432000) : ::Time.now.advance(days: -5)), mode = 1)\n    [since, mode]\n  end\nend\n";
    let outcome = Fixture::with_pass("lowered-default", original, lowered, &["lib/order.rb:2:22"], "lower-duration").verify();
    assert!(outcome.verified(), "{}", outcome.render());

    // A changed line after the defaults still fails.
    let elsewhere = lowered.replace("[since, mode]", "[mode, since]");
    let outcome = Fixture::with_pass("lowered-default-elsewhere", original, &elsewhere, &["lib/order.rb:2:22"], "lower-duration").verify();
    assert!(outcome.differences.iter().any(|difference| difference.contains("differs on line 3")), "{:?}", outcome.differences);
}
