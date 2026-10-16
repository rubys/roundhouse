//! What the emitted `boot.rb` carries for an app, beyond its own files.
//!
//! - The app's standard-library `require` lines. The emitted files do not
//!   carry their source's requires, so a library that adds methods rather
//!   than a constant (io/console's `IO#winsize=`) was never loaded: the
//!   constant-driven bundled requires cannot see it. A terminal channel
//!   resizes its pty with it.
//! - `Rails::VERSION`, as the app's locked Rails answers it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app_with_requires() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "app/models/terminal_probe.rb",
        "require \"io/console\" # IO#winsize= on the pty\nrequire \"pty\"\nrequire 'open3'\nrequire \"shellwords\"\nrequire \"not_a_stdlib_name\"\n\nclass TerminalProbe\n  def self.size = 1\nend\n",
    )
}

fn boot(target: BuildTarget) -> String {
    let (emitted, errors) = app_with_requires().emit(target);
    assert!(errors.is_empty(), "{target:?} errors: {errors:?}");
    std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb")
}

#[test]
fn the_apps_stdlib_requires_open_boot_on_both_trees() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let boot = boot(target);
        let head: String = boot.lines().take(12).collect::<Vec<_>>().join("\n");
        for lib in ["io/console", "pty", "open3"] {
            assert!(
                head.contains(&format!("require {lib:?}")),
                "{target:?}: boot.rb should require {lib} ahead of the app:\n{head}"
            );
        }
        assert!(
            !boot.contains("not_a_stdlib_name"),
            "{target:?}: only standard libraries are carried:\n{head}"
        );
    }
}

/// The spinel runtime defines `Shellwords` itself (without the package's
/// String reopen), so the app's `require "shellwords"` stays out there,
/// by the rule the bundled requires follow.
#[test]
fn a_library_the_tree_defines_itself_is_not_required() {
    let boot = boot(BuildTarget::Spinel);
    assert!(!boot.lines().any(|l| l.trim() == "require \"shellwords\""), "{boot}");
}

/// A library Spinel has no package for (`date`, `yaml`, ...) stays out
/// of the spinel tree, whose `--require-gate` build would refuse it, and
/// is carried on the ruby family.
#[test]
fn a_library_spinel_lacks_is_required_on_the_ruby_family_only() {
    let overlay = || {
        emit_and_run::real_blog().write(
            "app/models/dated_probe.rb",
            "require \"date\"\nrequire \"yaml\"\nrequire \"open3\"\n\nclass DatedProbe\n  def self.size = 1\nend\n",
        )
    };
    for (target, carried) in [(BuildTarget::Spinel, false), (BuildTarget::Ruby, true)] {
        let (emitted, errors) = overlay().emit(target);
        assert!(errors.is_empty(), "{target:?} errors: {errors:?}");
        let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb");
        assert!(boot.lines().any(|l| l.trim() == "require \"open3\""), "{target:?}:\n{boot}");
        for lib in ["date", "yaml"] {
            let line = format!("require {lib:?}");
            assert_eq!(boot.lines().take(12).any(|l| l.trim() == line), carried, "{target:?} {lib}:\n{boot}");
        }
    }
}

/// The library is loaded before the app runs: `IO#winsize=` exists only
/// once io/console is required.
#[test]
fn a_required_library_is_loaded_on_the_ruby_lane() {
    let run = app_with_requires().run_ruby("puts IO.method_defined?(:winsize=)\n");
    run.assert_passes();
    assert!(run.stdout.contains("true"), "{}", run.stdout);
}

/// A Packwerk package's files reach the emitted app as the root app's
/// do, so a require in one opens boot.rb too.
#[test]
fn a_package_s_requires_open_boot_too() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("packwerk.yml", "# package_paths: **/\n")
        .write("packs/terminal/package.yml", "enforce_dependencies: true\n")
        .write(
            "packs/terminal/app/services/terminal_size.rb",
            "require \"io/console\"\n\nclass TerminalSize\n  def self.rows = 1\nend\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "errors: {errors:?}");
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb");
    assert!(boot.lines().any(|l| l.trim() == "require \"io/console\""), "{boot}");
}

#[test]
fn rails_version_is_the_locked_one() {
    // fixtures/real-blog locks rails 8.1.4.
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let boot = boot(target);
        assert!(boot.contains("STRING = \"8.1.4\""), "{target:?}:\n{boot}");
        assert!(boot.contains("MAJOR = 8") && boot.contains("MINOR = 1") && boot.contains("TINY = 4"));
        // defined before the app's files load, as `require "rails"` does
        let version = boot.find("STRING = \"8.1.4\"").expect("version block");
        let app = boot.find("require_relative \"config/application\"").expect("config/application");
        assert!(version < app, "{target:?}: Rails::VERSION after the app loads:\n{boot}");
    }
    let run = app_with_requires().run_ruby("puts Rails::VERSION::STRING\nputs Rails::VERSION::PRE.inspect\n");
    run.assert_passes();
    assert!(run.stdout.contains("8.1.4\nnil"), "{}", run.stdout);
}
