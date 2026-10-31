//! `Klass.to_entity(self)` from an instance method `to_entity(load_associations: true)`.
//!
//! a large app's `Rollouts::Record#to_entity` delegates to the class
//! side `to_entity(rollout_record)`, which takes one POSITIONAL record. The
//! instance method of the same name takes a keyword. The emit wrote the
//! call `Klass.to_entity(**self)` and CRuby raised
//! `TypeError: no implicit conversion of Rollouts::Record into Hash`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const MODEL: &str = "module Optimizing
  class RolloutRecord < ApplicationRecord
    self.table_name = \"articles\"

    class << self
      private

      #: (RolloutRecord) -> String
      def build_entity(record)
        \"entity:#{record.title}\"
      end

      public

      # Avoid adding public methods other than the ones present for the creation of the entity.
      #: (RolloutRecord) -> String
      def to_entity(rollout_record)
        to_entities([rollout_record]).first.to_s
      end

      #: (Array[RolloutRecord]) -> Array[String]
      def to_entities(rollout_records)
        rollout_records.map { |r| build_entity(r) }
      end
    end

    #: (?load_associations: bool) -> String
    def to_entity(load_associations: true)
      return RolloutRecord.to_entity(self) if load_associations

      \"bare:#{title}\"
    end
  end
end
";

#[test]
fn a_positional_self_is_not_written_as_a_double_splat() {
    emit_and_run::real_blog()
        .write("app/models/optimizing/rollout_record.rb", MODEL)
        .run_ruby(
            r##"
e = Optimizing::RolloutRecord.new(title: "a")
raise e.to_entity.inspect unless e.to_entity == "entity:a"
raise e.to_entity(load_associations: false).inspect unless e.to_entity(load_associations: false) == "bare:a"
puts "positional self ok"
"##,
        )
        .assert_passes();
}
