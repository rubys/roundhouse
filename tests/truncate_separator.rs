//! `truncate(text, separator:)` — rubys/roundhouse#812.
//!
//! `ActionView::ViewHelpers.truncate` (the shared runtime every
//! ruby-family target calls through) only accepted `length:` and
//! `omission:`. `src/lower/view_to_library/helpers.rs`'s `Truncate`
//! arm hands every option through unchanged, so a view's
//! `truncate(text, length: 20, separator: " ")` passed `check` with no
//! gap and the emitted program raised `ArgumentError: unknown keyword:
//! :separator` on every request.
//!
//! Rails' real algorithm (`ActiveSupport::core_ext::String#truncate`,
//! read from the `activesupport` gem installed on this machine):
//!
//! ```text
//! cutoff = length - omission.length
//! return omission if cutoff <= 0
//! cutoff = rindex(separator, cutoff) || cutoff if separator
//! "#{self[0, cutoff]}#{omission}"
//! ```
//!
//! which for `"Rails is a web application framework"` at `length: 20,
//! separator: " "` cuts at the space before "application" (index 14),
//! giving `"Rails is a web..."` — confirmed against Rails 8.1.4's own
//! `String#truncate` below, not just read from source.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;
use std::process::Command;

use roundhouse::project::BuildTarget;

/// Rails 8.1.4's `String#truncate` answers; the last one's separator starts at the cutoff and runs past it.
const STRICT_CASES: &[(&str, i64, &str, &str)] = &[
    ("Rails is a web application framework", 20, " ", "Rails is a web..."),
    ("one two three four five", 12, " ", "one two..."),
    ("noseparatorhere", 8, " ", "nosep..."),
    ("ab cde fgh", 6, " cd", "ab..."),
];

fn run(dir: &Path, program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run {program}: {e}"));
    assert!(
        output.status.success(),
        "{program} {args:?} in {}\n{}\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn runtime_file() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/action_view/view_helpers.rb")
}

#[test]
fn truncate_separator_matches_rails_8_1_4() {
    let script = r#"
require "active_support"
require "active_support/core_ext/string/filters"
load ARGV.fetch(0)

def assert(value, message)
  raise message unless value
end

cases = [
  ["Rails is a web application framework", 20, "...", " "],
  ["one two three four five", 12, "...", " "],
  ["noseparatorhere", 8, "...", " "],
  ["short", 30, "...", " "],
]
cases.each do |text, length, omission, separator|
  runtime_result = ActionView::ViewHelpers.truncate(text, length: length, omission: omission, separator: separator)
  rails_result = text.truncate(length, omission: omission, separator: separator)
  assert(
    runtime_result == rails_result,
    "truncate(#{text.inspect}, length: #{length}, separator: #{separator.inspect}) " \
    "runtime=#{runtime_result.inspect} rails=#{rails_result.inspect}"
  )
end

assert(
  ActionView::ViewHelpers.truncate("Rails is a web application framework", length: 20, separator: " ") == "Rails is a web...",
  "must match the #812 reduction's own Rails 8.1.4 output"
)
puts "truncate separator contract passed"
"#;
    let output = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .arg(runtime_file())
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("contract passed"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
#[ignore = "requires the Rust toolchain; builds an emitted app"]
fn truncate_separator_runs_on_rust() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let asserts: String = STRICT_CASES
        .iter()
        .map(|(text, length, sep, want)| {
            format!("    assert_eq!(app::view_helpers::ViewHelpers::truncate({text:?}, {length}, \"...\", Some({sep:?}.to_string())), {want:?});\n")
        })
        .collect();
    std::fs::create_dir_all(emitted.join("tests")).unwrap();
    std::fs::write(emitted.join("tests/truncate_separator.rs"), format!("#[test]\nfn separator() {{\n{asserts}}}\n")).unwrap();
    run(&emitted, "cargo", &["test", "--quiet", "--test", "truncate_separator"]);
}

#[test]
#[ignore = "requires Go; builds an emitted app"]
fn truncate_separator_runs_on_go() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    let asserts: String = STRICT_CASES
        .iter()
        .map(|(text, length, sep, want)| {
            format!("\tif got := ActionViewViewHelpers_truncate({text:?}, int64({length}), \"...\", {sep:?}); got != {want:?} {{\n\t\tt.Errorf(\"got %q\", got)\n\t}}\n")
        })
        .collect();
    std::fs::write(
        emitted.join("app/v2/truncate_separator_test.go"),
        format!("package v2\n\nimport \"testing\"\n\nfunc TestTruncateSeparator(t *testing.T) {{\n{asserts}}}\n"),
    )
    .unwrap();
    run(&emitted, "go", &["mod", "tidy"]);
    run(&emitted, "go", &["test", "./app/v2/", "-run", "TestTruncateSeparator"]);
}

#[test]
#[ignore = "requires .NET; builds an emitted app"]
fn truncate_separator_runs_on_csharp() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let asserts: String = STRICT_CASES
        .iter()
        .map(|(text, length, sep, want)| {
            format!("        Assert.Equal({want:?}, ViewHelpers.Truncate({text:?}, {length}L, \"...\", {sep:?}));\n")
        })
        .collect();
    std::fs::write(
        emitted.join("tests/TruncateSeparatorTest.cs"),
        format!("using Xunit;\n\nnamespace Roundhouse;\n\npublic class TruncateSeparatorTest\n{{\n    [Fact]\n    public void Separator() {{\n{asserts}    }}\n}}\n"),
    )
    .unwrap();
    run(&emitted, "dotnet", &["test", "tests/App.Tests.csproj", "--filter", "FullyQualifiedName~TruncateSeparatorTest"]);
}
