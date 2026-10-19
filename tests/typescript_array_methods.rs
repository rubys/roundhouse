//! Array, number and String methods on the TypeScript emit. The emit translated a
//! handful (`each`, `map`, `size`, `sort_by`, …) and wrote every other
//! Ruby name on the JS value as-is: `xs.sum` and `n.odd?` read
//! `undefined`, `xs.select(...)` threw, `n <=> m` did not parse, and
//! nothing reported any of it. The common ones now translate (run against
//! CRuby in `typescript_toolchain::real_blog_{array,number,string}_methods_run_under_tsx`),
//! and the rest are refused by name.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/array_methods.rs"]
mod array_methods;

use roundhouse::project::BuildTarget;

#[test]
fn translated_array_number_and_string_methods_emit_without_errors() {
    let (_emitted, errors) = array_methods::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_untranslated_array_or_number_method_is_refused_on_typescript_only() {
    let probe = "class Untranslated\n  def self.run(xs, n, flags, by, records)\n    [xs.zip(xs), xs.tally, xs.group_by { |x| x > 1 }, n.digits, n.round(1), xs.all?, flags.any?, xs.select(&by), xs.pop(2), records.each_slice(1), records.partition { |r| r.id > 1 }]\n  end\nend\n";
    let app = || {
        emit_and_run::real_blog()
            .write("app/models/untranslated.rb", probe)
            .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @u = Untranslated.run([1, 2], 3, [true, nil], ->(x) { x > 1 }, Article.all.to_a)\n")
    };
    let (_emitted, errors) = app().emit(BuildTarget::Typescript);
    // `records` is an Array of Article from `to_a`, not a query: its `partition` is refused too.
    for name in ["Array#zip", "Array#tally", "Array#group_by", "Integer#digits", "Integer#round", "Array#all?", "Array#any?", "Array#select", "Array#pop", "Array#partition"] {
        assert!(errors.iter().any(|e| e.contains(name)), "{name}: {errors:?}");
    }
    let (_emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_untranslated_string_method_is_refused_on_typescript_only() {
    // A spec only known at run time, an empty pad Ruby raises on, and names JS strings lack.
    let probe = "class Untranslated\n  def self.run(s, t)\n    [s.count(t), s.tr(t, \"x\"), s.center(4, \"\"), s.swapcase, s.succ, s.lines, s.scan(/l/), s =~ /l/, s.split(\",\", 2), s.to_f, s.split(t), s.split(/l/)]\n  end\nend\n";
    let app = || {
        emit_and_run::real_blog()
            .write("app/models/untranslated.rb", probe)
            .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @u = Untranslated.run(\"hello\", \"l\")\n")
    };
    let (_emitted, errors) = app().emit(BuildTarget::Typescript);
    for name in ["String#count", "String#tr", "String#center", "String#swapcase", "String#succ", "String#lines", "String#scan", "String#=~", "String#split", "String#to_f"] {
        assert!(errors.iter().any(|e| e.contains(name)), "{name}: {errors:?}");
    }
    // `split` with a limit, a separator only known at run time, and a regex.
    assert_eq!(errors.iter().filter(|e| e.contains("String#split")).count(), 3, "{errors:?}");
    let (_emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
}
