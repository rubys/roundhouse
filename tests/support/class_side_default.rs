//! One contract for a class method's parameter default that calls a
//! sibling class method, shared by the interpreted (CRuby overlay) and
//! native (spinel) lanes. A bare name in a default is read on `self`,
//! which in a class method is the class: `def open(path = default_path)`
//! under `class << self` calls the class's own `default_path`. The
//! dead-default pass looked among instance methods only and turned the
//! default into `raise "undefined local variable or method 'default_path'"`.
//!
//! The expected lines are CRuby 4.0's output for the same code.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().write("app/models/path_index.rb", SOURCE)
}

const SOURCE: &str = r##"class PathIndex
  class << self
    def default_path
      "/var/index"
    end

    def open(path = default_path, registry: "/var/registry")
      "#{path}|#{registry}"
    end
  end

  def self.label(name = default_label)
    name
  end

  def self.default_label
    "untitled"
  end
end
"##;

pub const SCRIPT: &str = r##"
puts PathIndex.open
puts PathIndex.open("/tmp/x")
puts PathIndex.label
"##;

pub const EXPECTED: &str = "/var/index|/var/registry\n/tmp/x|/var/registry\nuntitled\n";
