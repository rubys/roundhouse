//! A class whose body declares a constant of each literal kind, and a
//! class method reading them all (#1223).

use super::emit_and_run::{self, Overlay};

pub const SCALE: &str = r#"class Scale
  WEIGHTS = { "small" => 1, "large" => 9 }.freeze
  NAMES = %w[small large].freeze
  LABEL = "scale"
  LIMIT = 9
  RATIO = 1.5
  FLAGS = [true, false, nil].freeze
  NESTED = { "a" => [1, 2], "b" => { "c" => "d" } }.freeze

  def self.summary
    [WEIGHTS.fetch("large", 0), NAMES.first, LABEL, LIMIT, RATIO, FLAGS.length, NESTED["b"]["c"], NESTED["a"][1]].join(",")
  end
end
"#;

/// A model whose methods read its own constants through the Hash and
/// Array methods the TypeScript emit rewrites (#1223 b).
pub const GAUGE: &str = r#"class Gauge < ApplicationRecord
  WEIGHTS = { "small" => 1, "large" => 9, "none" => nil }.freeze
  NAMES = %w[small large].freeze

  def summary
    [WEIGHTS.fetch(size, 0), WEIGHTS.key?(size), WEIGHTS.keys.join("|"), NAMES.first, WEIGHTS[size]].join(",")
  end

  def self.heaviest
    WEIGHTS.fetch("large", 0)
  end

  # A key holding nil answers nil, not the default; an Object.prototype
  # name is not a key.
  def self.probe
    [WEIGHTS.fetch("none", 0).nil?, WEIGHTS.key?("constructor"), WEIGHTS.key?("none")].join(",")
  end
end
"#;

/// What `Gauge#summary` answers under CRuby for a `size` of "large".
#[allow(dead_code)]
pub const GAUGE_SUMMARY: &str = "9,true,small|large|none,small,9";

pub fn gauge_overlay() -> Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|\n",
            "  create_table \"gauges\", force: :cascade do |t|\n    t.string \"size\"\n  end\n\n  create_table \"articles\", force: :cascade do |t|\n",
        )
        .write("app/models/gauge.rb", GAUGE)
}

/// A child class whose constant shadows its parent's, with a value of
/// another kind (Ruby: `Dial::WEIGHTS` is "s", `Meter::WEIGHTS` the Hash).
pub const METER: &str = "class Meter\n  WEIGHTS = { \"a\" => 1 }.freeze\nend\n";
pub const DIAL: &str = "class Dial < Meter\n  WEIGHTS = \"s\"\n\n  def self.summary\n    [WEIGHTS, Meter::WEIGHTS[\"a\"]].join(\",\")\n  end\nend\n";

/// What `Dial.summary` answers under CRuby.
#[allow(dead_code)]
pub const SHADOW_SUMMARY: &str = "s,1";

pub fn shadow_overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/meter.rb", METER).write("app/models/dial.rb", DIAL)
}

/// What `Scale.summary` answers under CRuby.
#[allow(dead_code)]
pub const SUMMARY: &str = "9,small,scale,9,1.5,3,d,2";

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/scale.rb", SCALE)
}
