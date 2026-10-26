//! Regression found by CodeRabbit on roundhouse PR #1034 (review round
//! 3): `Relation#windowed_cache_version` qualified its timestamp
//! projection against `@table` — the model's REAL table name — rather
//! than the active FROM source. `Widget.from("widgets AS recent").
//! select("recent.*").limit(1)` aliases the table to `recent`; nothing
//! in that query's own FROM clause still answers to `widgets`, so the
//! window's `"widgets.updated_at AS __rh_cache_ts"` projection failed
//! with an unknown-column error on every `cache_key_with_version` call
//! for this shape, caching on or off.
//!
//! Bounded fix (lead decision, same policy as the grouped-relation
//! refusal): refuse clearly with `ArgumentError` naming the shape,
//! rather than trying to resolve the projection against an arbitrary
//! FROM alias or subquery.
//!
//! NOT tested here: the non-windowed path. `maximum`/`extreme` has
//! this same `@table`-vs-`from_source` mistake for a custom `from`
//! with no window — pre-existing, general-purpose code outside this
//! PR, filed separately rather than folded in here.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn widget_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
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
}

#[test]
fn a_custom_from_source_with_a_window_refuses_clearly() {
    let run = widget_app().run_ruby(
        "Widget.create!(name: \"a\")\nbegin\n  Widget.from(\"widgets AS recent\").select(\"recent.*\").limit(1).cache_key_with_version\n  puts \"NO_RAISE\"\nrescue ArgumentError => e\n  puts \"ARGUMENT_ERROR: #{e.message}\"\nrescue StandardError => e\n  puts \"OTHER_ERROR: #{e.class}: #{e.message}\"\nend\n",
    );
    run.assert_passes();
    assert!(
        run.stdout.contains("ARGUMENT_ERROR"),
        "a custom FROM source under a limit/offset window must refuse clearly (ArgumentError), not fail with an unknown-column SQL error (or any other opaque error): stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
