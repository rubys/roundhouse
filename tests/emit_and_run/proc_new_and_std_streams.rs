//! `Proc.new { … }` is Kernel's `proc { … }`, and `STDOUT` / `STDERR`
//! are the process's own streams (discourse writes its warnings through
//! both).

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "app/models/reporter.rb",
        "class Reporter\n  def self.doubled(n)\n    Proc.new { |x| x * 2 }.call(n)\n  end\n\n  def self.report(n)\n    STDERR.puts \"reporting #{n}\"\n    STDOUT.puts \"report: #{doubled(n)}\"\n    STDOUT.print \"done\\n\"\n    n\n  end\nend\n",
    )
}

const ASSERTIONS: &str = r##"got = [Reporter.doubled(21), Reporter.report(4)]
raise got.inspect unless got == [42, 4]
puts "proc new passed"
"##;

#[test]
fn proc_new_and_std_streams_run() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("report: 8\ndone\n"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn proc_new_and_std_streams_run_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("report: 8\ndone\n"), "{}", run.stdout);
}

/// No strict emit maps Ruby's process streams (each spelled `STDERR` /
/// `$stderr` as a name its runtime does not define); they are reported
/// there.
#[test]
fn std_streams_are_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = emit_and_run::real_blog()
            .edit(
                "app/models/article.rb",
                "  validates :title, presence: true\n",
                "  validates :title, presence: true\n\n  def self.report(n)\n    STDERR.puts \"r #{n}\"\n    $stdout.puts \"s\"\n    n\n  end\n",
            )
            .emit(target);
        for name in ["`STDERR` is Ruby's process stream", "`$stdout` is Ruby's process stream"] {
            assert!(errors.iter().any(|e| e.contains(name)), "{target:?} {name}: {errors:#?}");
        }
    }
}
