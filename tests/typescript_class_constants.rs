//! #1223: the TypeScript emit declares a class's own constants. A
//! literal value (`nil`, a Boolean, number, String, Symbol or regexp, or
//! an Array or Hash of those, `.freeze`d or not) becomes a `static`
//! member, so a `Klass.NAME` read resolves; before, nothing declared it,
//! `tsc` rejected every read (TS2339) and it was `undefined` at run
//! time. The run itself is
//! `typescript_toolchain::real_blog_class_constants_run_under_tsx`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/class_constants.rs"]
mod class_constants;

use roundhouse::project::BuildTarget;

#[test]
fn literal_constants_are_declared_as_static_members() {
    let (emitted, errors) = class_constants::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let scale = std::fs::read_to_string(emitted.join("app/models/scale.ts")).expect("scale.ts");
    for decl in [
        "static WEIGHTS = { \"small\": 1, \"large\": 9 };",
        "static NAMES = [\"small\", \"large\"];",
        "static LABEL = \"scale\";",
        "static LIMIT = 9;",
        "static RATIO = 1.5;",
        "static FLAGS = [true, false, null];",
        "static NESTED = { \"a\": [1, 2], \"b\": { \"c\": \"d\" } };",
    ] {
        assert!(scale.contains(decl), "{decl}:\n{scale}");
    }
}

/// A value that is not a literal (a constant read, a call) is not
/// declared, nor is a constant whose name a class method also takes.
#[test]
fn only_literal_unclaimed_constants_are_declared() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/gauge.rb",
            "class Gauge\n  WEIGHTS = { \"a\" => 1 }.freeze\n  ALIAS = WEIGHTS\n  STARTED = Time.now\n  LEVEL = 3\n  MODE = \"auto\"\n\n  def self.LEVEL\n    4\n  end\n\n  def MODE\n    \"manual\"\n  end\n\n  def self.size\n    WEIGHTS.size\n  end\nend\n",
        )
        .emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let gauge = std::fs::read_to_string(emitted.join("app/models/gauge.ts")).expect("gauge.ts");
    assert!(gauge.contains("static WEIGHTS = { \"a\": 1 };"), "{gauge}");
    // An instance method of the same name does not collide: TypeScript
    // keeps the static and instance sides apart, as Ruby does.
    assert!(gauge.contains("static MODE = \"auto\";"), "{gauge}");
    for absent in ["static ALIAS", "static STARTED", "static LEVEL ="] {
        assert!(!gauge.contains(absent), "{absent}:\n{gauge}");
    }
}

/// A model's own constant reads carry their value's type through the
/// model lowering (#1223 b), so the Hash and Array rewrites apply as they
/// do in a plain class. The read was typed as a phantom `Gauge::WEIGHTS`
/// class, and `fetch`, `key?` and `keys` reached a plain object.
#[test]
fn a_models_constant_reads_get_the_hash_and_array_rewrites() {
    let (emitted, errors) = class_constants::gauge_overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let gauge = std::fs::read_to_string(emitted.join("app/models/gauge.ts")).expect("gauge.ts");
    for read in [
        "const h = Gauge.WEIGHTS; const k = this.size; const d = 0; return Object.prototype.hasOwnProperty.call(h, k) ? h[k] : d;",
        "Object.prototype.hasOwnProperty.call(Gauge.WEIGHTS, this.size)",
        "Object.keys(Gauge.WEIGHTS)",
        "Gauge.NAMES[0]",
    ] {
        assert!(gauge.contains(read), "{read}:\n{gauge}");
    }
    assert!(!gauge.contains("Gauge.WEIGHTS.fetch") && !gauge.contains("is_key"), "{gauge}");
    // A source `def self.heaviest` reads it the same way.
    assert!(gauge.contains("const h = Gauge.WEIGHTS; const k = \"large\"; const d = 0;"), "{gauge}");
}

/// A `.freeze` nested inside a literal constant answers its receiver:
/// `{ "a" => [1, 2].freeze }.freeze` declares `{ "a": [1, 2] }`. The
/// nested `.freeze` was emitted as a property read, which is `undefined`.
#[test]
fn a_nested_freeze_in_a_literal_constant_answers_its_receiver() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/rack.rb",
            "class Rack\n  NESTED = { \"a\" => [1, 2].freeze, \"b\" => { \"c\" => \"d\" }.freeze }.freeze\n\n  def self.first_a\n    NESTED[\"a\"][0]\n  end\nend\n",
        )
        .emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let rack = std::fs::read_to_string(emitted.join("app/models/rack.ts")).expect("rack.ts");
    assert!(rack.contains("static NESTED = { \"a\": [1, 2], \"b\": { \"c\": \"d\" } };"), "{rack}");
    assert!(!rack.contains(".freeze"), "{rack}");
}

/// A constant shadowing an ancestor's static of the same name is typed
/// `any`: TypeScript requires a child's static side to be assignable to
/// its parent's (TS2417), and the Ruby values may be of different kinds.
#[test]
fn a_constant_shadowing_an_ancestors_static_is_typed_any() {
    let (emitted, errors) = class_constants::shadow_overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let dial = read("app/models/dial.ts");
    let meter = read("app/models/meter.ts");
    assert!(dial.contains("static WEIGHTS: any = \"s\";"), "{dial}");
    assert!(meter.contains("static WEIGHTS = { \"a\": 1 };"), "{meter}");
}

/// Each constant the TypeScript emit leaves out is reported for that
/// target, saying why (#1223 c): a value that is not a literal, or a
/// name a static method or field of the class also takes. The emit still writes
/// the tree (a warning, as the per-target `uniqueness:` gap is), and the
/// ruby target, which keeps every constant, reports none.
#[test]
fn the_cli_reports_each_constant_the_typescript_emit_leaves_out() {
    let root = std::env::temp_dir().join(format!("roundhouse-ts-constants-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/gauge.rb", "class Gauge\n  WEIGHTS = { \"a\" => 1 }.freeze\n  ALIAS = WEIGHTS\n  STARTED = Time.now\n  LEVEL = 3\n\n  def self.LEVEL\n    4\n  end\n\n  def self.size\n    WEIGHTS.size\n  end\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ] {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let run = |target: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target, "-o"])
            .arg(root.join(format!("out-{target}")))
            .arg(root.join("app_src"))
            .output()
            .expect("run roundhouse");
        let lines: Vec<String> = String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| l.contains("warning[unsupported]: constant Gauge::"))
            .map(String::from)
            .collect();
        (out.status.success(), lines)
    };
    let (ts_ok, ts) = run("typescript");
    let (rb_ok, rb) = run("ruby");
    std::fs::remove_dir_all(&root).ok();
    assert!(ts_ok && rb_ok, "the emits still write the tree: {ts:#?}");
    assert_eq!(ts.len(), 3, "{ts:#?}");
    let line = |name: &str| ts.iter().find(|l| l.contains(&format!("constant Gauge::{name} "))).cloned().unwrap_or_default();
    assert!(line("ALIAS").contains("gauge.rb:3:") && line("ALIAS").contains("read finds nothing: its value is not a literal"), "{ts:#?}");
    assert!(line("STARTED").contains("gauge.rb:4:") && line("STARTED").contains("not a literal"), "{ts:#?}");
    assert!(line("LEVEL").contains("gauge.rb:5:") && line("LEVEL").contains("a `Gauge.LEVEL` read reaches the class's static method or field of that name instead"), "{ts:#?}");
    assert!(rb.is_empty(), "{rb:#?}");
}
