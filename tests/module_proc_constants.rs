//! A constant whose value is `proc { … }` or `lambda { … }` (Kernel#proc,
//! Kernel#lambda with a literal block) is the Proc the block becomes, as
//! `->(x) { … }` and `Proc.new { … }` already were. Typed as no value, every
//! read of it was "constant not supported" and emitted as a raise.
//! app_profiler 0.6.0 declares
//!
//! ```ruby
//! module AppProfiler
//!   DefaultProfileFormatter = proc do |upload| … end
//!   mattr_reader :profile_url_formatter, default: DefaultProfileFormatter
//! ```
//!
//! and the emitted `@@profile_url_formatter = (raise "roundhouse: constant
//! not supported (all targets)")` stopped a large app's boot when the module
//! loaded.
//!
//! The control is CRuby running the same source against the pinned
//! ActiveSupport (tests/support/rails_oracle.rb).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const ORACLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");

fn native(program: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rh_module_proc_constants_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.rb"), format!("require {ORACLE:?}\n{program}")).unwrap();
    let out = Command::new("ruby").arg("main.rb").current_dir(&dir).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

const PRELUDE: &str = "RailsOracle.activate(\"activesupport\")\nrequire \"active_support\"\n\
     require \"active_support/core_ext/module/attribute_accessors\"\n";

/// No emitted file may stand in a raise for a constant read.
fn assert_no_constant_raise(emitted: &std::path::Path) {
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rb") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for line in text.lines().filter(|l| l.contains("constant not supported")) {
                    hits.push(format!("{}: {line}", path.display()));
                }
            }
        }
    }
    let mut hits = Vec::new();
    walk(&emitted.join("app"), &mut hits);
    assert!(hits.is_empty(), "emitted constant raises:\n{}", hits.join("\n"));
}

/// The shape app_profiler has: proc constants, then `mattr_*` defaults
/// naming them, in one module body.
const PROFILER: &str = r##"module Profiler
  Fmt = proc { |x| x * 2 }
  Prefix = proc do
    "p"
  end

  mattr_reader :fmt, default: Fmt
  mattr_accessor :prefix, default: Prefix
end
"##;

#[test]
fn a_proc_constant_is_a_mattr_default_as_native() {
    let script = "p Profiler.fmt.call(2)\np Profiler.prefix.call\np Profiler.fmt.equal?(Profiler::Fmt)\n\
                  p [Profiler::Fmt.lambda?, Profiler::Prefix.arity]\n\
                  Profiler.prefix = proc { \"q\" }\np Profiler.prefix.call\n";
    let expected = native(&format!("{PRELUDE}{PROFILER}{script}"));
    assert_eq!(expected, "4\n\"p\"\ntrue\n[false, 0]\n\"q\"\n", "native control");
    let run = emit_and_run::empty_app().write("lib/profiler.rb", PROFILER).run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, expected, "{}", run.stderr);
    assert_no_constant_raise(&run.emitted);
}

/// Sibling code reads the same constants: a nested class's instance and
/// singleton methods, the module's own methods, a `cattr_reader` default
/// in a class, and `lambda do … end` beside `proc`.
const FORMATS: &str = r##"module Formats
  Triple = proc { |x| x * 3 }
  Label = lambda do |name|
    "<#{name}>"
  end

  mattr_reader :triple, default: Triple

  def self.label_of(name)
    Label.call(name)
  end

  class Consumer
    cattr_reader :wrap, default: Label

    def run(v)
      Triple.call(v)
    end

    def self.label
      Label[:c]
    end
  end
end
"##;

#[test]
fn proc_and_lambda_constants_are_read_by_sibling_code_as_native() {
    let script = "p Formats.triple.call(2)\np Formats.label_of(:m)\np Formats::Consumer.new.run(4)\n\
                  p Formats::Consumer.label\np Formats::Consumer.wrap.call(:w)\n\
                  p [Formats::Label.lambda?, Formats::Triple.lambda?]\n";
    let expected = native(&format!("{PRELUDE}{FORMATS}{script}"));
    assert_eq!(expected, "6\n\"<m>\"\n12\n\"<c>\"\n\"<w>\"\n[true, false]\n", "native control");
    let run = emit_and_run::empty_app().write("lib/formats.rb", FORMATS).run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, expected, "{}", run.stderr);
    assert_no_constant_raise(&run.emitted);
}
