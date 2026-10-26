//! `def self.call(...) = new(...).call` and the nearby anonymous packets.
//!
//! Optional-keyword `initialize` used to flatten to positionals, so the
//! packet was refused or bound as one argument. The lowered definition
//! keeps the keywords, and the emitted Ruby runs the packet unchanged.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::session::analyze_and_lower;

fn check_errors(source: &str) -> Vec<String> {
    let tree: HashMap<PathBuf, Vec<u8>> = [(
        PathBuf::from("app/services/probe.rb"),
        source.as_bytes().to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect()
}

fn app_from(source: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [(
        PathBuf::from("app/services/probe.rb"),
        source.as_bytes().to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let lower = analyze_and_lower(&mut app);
    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    app
}

fn emitted(app: &roundhouse::App) -> String {
    // Drop the emitted `require_relative` lines: this runs as one script,
    // not as the emitted tree. Parents sort before subclasses.
    let mut parts: Vec<String> = ruby::emit_library(app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|ext| ext == "rb"))
        .map(|f| {
            f.content
                .lines()
                .filter(|line| !line.contains("require_relative"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    parts.sort_by_key(|part| part.contains("class Child"));
    parts.join("\n")
}

fn ruby_eval(source: &str, script: &str) -> String {
    let output = Command::new("ruby")
        .args(["-e", &format!("{source}\n{script}")])
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn constant_callee_restores_keywords_without_a_typed_receiver() {
    let source = r#"
class Flat
  def initialize(scale: 2)
    @scale = scale
  end
  def call
    @scale
  end
end
class Other
  def self.anon(**)
    Flat.new(**).call
  end
end
module Billing
  class Scale
    def initialize(scale: 9)
      @scale = scale + 10
    end
    def call
      @scale
    end
  end
  class Other
    def self.anon(**)
      Scale.new(**).call
    end
  end
end
class Scale
  def initialize(scale: 1)
    @scale = scale
  end
  def call
    @scale
  end
end
"#;
    let errors = check_errors(source);
    assert!(errors.is_empty(), "{errors:?}");
    let app = app_from(source);
    let out = emitted(&app);
    assert!(out.contains("def initialize(scale:"), "{out}");
    assert!(out.contains("Flat.new(**)"), "{out}");
    assert!(out.contains("Scale.new(**)"), "{out}");
    let got = ruby_eval(
        &out,
        "puts Other.anon(scale: 6)\nputs Other.anon\nputs Billing::Other.anon(scale: 3)\nputs Billing::Other.anon\n",
    );
    assert_eq!(got, "6\n2\n13\n19\n", "{out}");
}

#[test]
fn qualified_class_constant_uses_ruby_nesting() {
    // `class Billing::Other` nests only that class, so `Scale` is `::Scale`.
    // `module Billing; class Other` would see `Billing::Scale`.
    let source = r#"
class Scale
  def initialize(scale: 1)
    @scale = scale
  end
  def call
    @scale
  end
end
class Billing::Scale
  def initialize(scale: 9)
    @scale = scale + 10
  end
  def call
    @scale
  end
end
class Billing::Other
  def self.anon(**)
    Scale.new(**).call
  end
end
"#;
    let errors = check_errors(source);
    assert!(errors.is_empty(), "{errors:?}");
    let out = emitted(&app_from(source));
    assert!(
        out.contains("def initialize(scale: 1)") || out.contains("def initialize(scale:1)"),
        "the callee keeps its keyword:\n{out}"
    );
    assert!(
        !out.contains("scale: 9") && !out.contains("scale:9"),
        "the unreached namesake stays flattened:\n{out}"
    );
    let got = ruby_eval(&out, "puts Billing::Other.anon(scale: 3)\nputs Billing::Other.anon\n");
    assert_eq!(got, "3\n1\n", "{out}");
}

#[test]
fn check_restores_constant_new_before_typing() {
    let source = r#"
class Probe
  def self.call(...) = new(...).call
  def initialize(scale: 2)
    @scale = scale
  end
  def call
    @scale
  end
end
class Other
  def self.anon(**)
    Probe.new(**).call
  end
end
"#;
    let errors = check_errors(source);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn service_call_forwards_keywords_and_a_block_into_initialize() {
    let source = r#"
class Probe
  def self.call(...) = new(...).call

  def initialize(label, scale: 2)
    @label = label
    @scale = scale
  end

  def call
    shown = @label.to_s
    shown = yield(shown) if block_given?
    shown * @scale
  end
end
"#;
    let app = app_from(source);
    let out = emitted(&app);
    assert!(out.contains("def self.call(...)"), "{out}");
    assert!(out.contains("new(...).call"), "{out}");
    assert!(out.contains("def initialize(label, scale:"), "{out}");
    // `new(...).call` forwards the block to `new`, as Ruby does. The
    // keyword and the leading positional still reach `initialize`.
    let got = ruby_eval(
        &out,
        "puts Probe.call(\"ab\", scale: 3) { |s| s.upcase }\nputs Probe.call(\"ab\")\n",
    );
    assert_eq!(got, "ababab\nabab\n");
}

#[test]
fn anonymous_keyword_forward_keeps_optional_keywords_and_ordered_pairs() {
    let source = r##"
class Probe
  def self.call(**) = new(**).call

  def self.tagged(**)
    new(kind: :note, **).call
  end

  def initialize(kind: :plain, scale: 2)
    @kind = kind
    @scale = scale
  end

  def call
    "#{@kind}-#{@scale}"
  end
end
"##;
    let app = app_from(source);
    let out = emitted(&app);
    assert!(out.contains("def self.call(**)"), "{out}");
    assert!(out.contains("new(**).call"), "{out}");
    assert!(out.contains("new(kind: :note, **)"), "{out}");
    assert!(out.contains("kind:"), "{out}");
    assert!(out.contains("scale:"), "{out}");
    let got = ruby_eval(
        &out,
        "puts Probe.call(scale: 4)\nputs Probe.tagged(scale: 5)\nputs Probe.call\n",
    );
    assert_eq!(got, "plain-4\nnote-5\nplain-2\n");
}

#[test]
fn full_forward_reaches_optional_keywords_keyword_rest_and_implicit_super() {
    for (source, script, expected) in [
        (
            "class Probe\n def call(...)\n target(...)\n end\n def target(a, b, factor: 2)\n (a-b)*factor\n end\nend",
            "puts Probe.new.call(11, 4, factor: 3)\nputs Probe.new.call(11, 4)\n",
            "21\n14\n",
        ),
        (
            "class Probe\n def call(...)\n self.target(...)\n end\n def target(a, b, **kw)\n (a-b)*kw[:factor]\n end\nend",
            "puts Probe.new.call(11, 4, factor: 3)\n",
            "21\n",
        ),
        (
            "class Parent\n def call(a, b, factor: 2)\n (a-b)*factor\n end\nend\nclass Child < Parent\n def call(...)\n super\n end\nend",
            "puts Child.new.call(11, 4, factor: 3)\nputs Child.new.call(11, 4)\n",
            "21\n14\n",
        ),
        (
            "class Probe\n def target(**options)\n options[:factor]\n end\n def call(**)\n target(**)\n end\nend",
            "puts Probe.new.call(factor: 3)\n",
            "3\n",
        ),
        (
            "module Mixed\n def call(a, b, factor: 2)\n (a-b)*factor\n end\nend\nclass Parent\n def call(a,b,factor:)\n (a-b)*factor\n end\nend\nclass Child < Parent\n include Mixed\n def call(...)\n super(...)\n end\nend",
            "puts Child.new.call(11, 4, factor: 3)\nputs Child.new.call(11, 4)\n",
            "21\n14\n",
        ),
        (
            "class Parent\n def self.build(...)\n new(...)\n end\n def initialize(factor:)\n @factor=factor\n end\n def factor\n @factor\n end\nend\nclass Child < Parent\n def initialize(factor: 2)\n @factor=factor\n end\nend",
            "puts Child.build(factor: 3).factor\nputs Child.build.factor\n",
            "3\n2\n",
        ),
    ] {
        let app = app_from(source);
        let got = ruby_eval(&emitted(&app), script);
        assert_eq!(got, expected, "{source}\n{}", emitted(&app));
    }
}

#[test]
fn inherited_optional_initialize_accepts_the_subclass_packet() {
    let source = r#"
class Parent
  def self.call(...) = new(...).call
  def initialize(scale: 2)
    @scale = scale
  end
  def call
    @scale
  end
end
class Child < Parent
  def initialize(scale: 7)
    @scale = scale + 1
  end
end
"#;
    let app = app_from(source);
    let out = emitted(&app);
    assert!(out.contains("def initialize(scale:"), "{out}");
    let got = ruby_eval(&out, "puts Child.call(scale: 3)\nputs Parent.call\n");
    assert_eq!(got, "4\n2\n");
}

#[test]
fn named_splat_into_a_restored_initializer_still_binds() {
    let source = r#"
class Probe
  def self.call(...) = new(...).call
  def self.via(opts)
    new(**opts).call
  end
  def initialize(scale: 2)
    @scale = scale
  end
  def call
    @scale
  end
end
class Other
  def self.via(opts)
    Probe.new(**opts).call
  end
  def self.anon(**)
    Probe.new(**).call
  end
end
"#;
    let app = app_from(source);
    let out = emitted(&app);
    let got = ruby_eval(
        &out,
        "puts Probe.via({scale: 4})\nputs Probe.call(scale: 3)\nputs Other.via({scale: 5})\nputs Other.anon(scale: 6)\n",
    );
    assert_eq!(got, "4\n3\n5\n6\n", "{out}");
}
