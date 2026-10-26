//! Regression found by CodeRabbit on roundhouse PR #1034:
//! `Relation#compute_cache_version` called `maximum(timestamp_column)`
//! unconditionally. `maximum` (`extreme`) answers a Hash, not a
//! scalar, for a GROUPED relation (`group(:name).maximum(:updated_at)`
//! is one extreme PER GROUP, matching Rails) — so
//! `timestamp.to_f`/`.to_i` on it raised `NoMethodError`, a confusing
//! error with no useful message, on every `cache_key_with_version`
//! call for a grouped relation.
//!
//! Real Rails' own `compute_cache_version` has no GROUP BY case either
//! — its aggregate SQL answers one row PER GROUP and its `rows.first`
//! picks an arbitrary one, not a contract worth reproducing (which
//! group "wins" is unspecified). This fix refuses clearly instead:
//! `ArgumentError`, not `NoMethodError` on an unrelated Hash method.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_grouped_relations_cache_version_refuses_clearly() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .run_ruby(
            "Widget.create!(name: \"a\")\nbegin\n  Widget.group(:name).cache_key_with_version\n  puts \"NO_RAISE\"\nrescue ArgumentError => e\n  puts \"ARGUMENT_ERROR: #{e.message}\"\nrescue NoMethodError => e\n  puts \"NO_METHOD_ERROR: #{e.message}\"\nend\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("ARGUMENT_ERROR"),
        "a grouped relation's cache_key_with_version must refuse clearly (ArgumentError), not raise NoMethodError on a Hash, nor silently answer a wrong value: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
