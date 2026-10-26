//! A subclass of a tableless `ActiveModel::Model` form
//! (`class Form::Sub < Form::Base`). activemodel 8.1.4: the child inherits
//! the parent's attributes, validations and constructor and adds its own;
//! `Sub.new(count: "7", extra: "8")` casts both, a parent validation still
//! runs, and an `attribute` the child redeclares overrides the parent's
//! type for the child only.
//!
//! The child was ingested as a library class, which dropped its own
//! `attribute` and `validates` ("not modelled and is dropped").
use super::emit_and_run;

const BASE: &str = r##"class Form::Base
  include ActiveModel::Model
  include ActiveModel::Attributes

  attr_accessor :kind
  attribute :count, :integer
  attribute :flag, :boolean

  validates :kind, presence: true

  def describe
    "#{self.class.name} #{count.inspect} #{flag.inspect}"
  end
end
"##;

const SUB: &str = r#"class Form::Sub < Form::Base
  attr_accessor :target
  attribute :extra, :integer

  validates :target, presence: true
end
"#;

const RETYPED: &str = r#"class Form::Retyped < Form::Base
  attribute :count, :string
end
"#;

const PLAIN: &str = r#"class Form::Plain < Form::Base
end
"#;

const LEAF: &str = r#"module Form
  class Leaf < Sub
    attribute :depth, :integer
  end
end
"#;

const SCRIPT: &str = r##"
require_relative "app/models/form/base"
require_relative "app/models/form/sub"
require_relative "app/models/form/retyped"
require_relative "app/models/form/plain"
require_relative "app/models/form/leaf"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
s = Form::Sub.new(count: "7", extra: "8", flag: "0")
check("inherited cast", s.count, 7)
check("own cast", s.extra, 8)
check("inherited boolean", s.flag, false)
check("both valid", Form::Sub.new(kind: "k", target: "t").valid?, true)
check("parent validation", Form::Sub.new(target: "t").valid?, false)
own = Form::Sub.new(kind: "k")
check("own validation", own.valid?, false)
check("own validation message", own.errors, ["Target can't be blank"])
check("parent method", Form::Sub.new(count: "3").describe, "Form::Sub 3 nil")
s.extra = "09"
s.count = "4"
check("writers cast", [s.count, s.extra], [4, 9])
check("retyped", Form::Retyped.new(count: 12).count, "12")
check("parent keeps its type", Form::Base.new(count: "12").count, 12)
check("no own attributes", Form::Plain.new(count: "5").count, 5)
leaf = Form::Leaf.new(count: "1", extra: "2", depth: "3")
check("two levels", [leaf.count, leaf.extra, leaf.depth], [1, 2, 3])
check("two levels validate", Form::Leaf.new(kind: "k").valid?, false)
puts "ActiveModel superclass passed"
"##;

fn forms() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/form/base.rb", BASE)
        .write("app/models/form/sub.rb", SUB)
        .write("app/models/form/retyped.rb", RETYPED)
        .write("app/models/form/plain.rb", PLAIN)
        .write("app/models/form/leaf.rb", LEAF)
}

#[test]
fn a_tableless_subclass_inherits_its_parents_attributes_on_ruby() {
    let run = forms().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel superclass passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_tableless_subclass_inherits_its_parents_attributes_on_spinel() {
    let run = forms().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ActiveModel superclass passed"), "{}", run.stdout);
}
