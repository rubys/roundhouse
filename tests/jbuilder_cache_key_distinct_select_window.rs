//! Regression found by CodeRabbit on roundhouse PR #1034 (review round
//! 3): `Relation#windowed_cache_version` replaces the projection with
//! the primary key (plus the timestamp) under a limit/offset window,
//! even when the relation already carries its own explicit
//! `select(...)` combined with DISTINCT. A pk is already unique, so
//! substituting it makes the DISTINCT a no-op — the window then covers
//! whichever rows the plain, un-deduplicated LIMIT happens to land on,
//! not the rows the real DISTINCT-projected query returns.
//!
//! `Widget.select(:name).distinct.order(:id).limit(2)` with widgets
//! `a` (id 1), `a2` (id 2, same name "a") and `b` (id 3): the real
//! query's DISTINCT names, in id order, are "a" then "b" — `b` is
//! part of the page. The buggy window instead counts/maxes over the
//! first two PHYSICAL rows (`a`, `a2`), since DISTINCT on a pk changes
//! nothing; `b` never enters it. Touching `b` then left the version
//! unchanged — a stale fragment, silently.
//!
//! Bounded fix (lead decision, same policy as the grouped-relation and
//! custom-FROM refusals): refuse clearly with `ArgumentError` rather
//! than compute a version for a different window than the real query.
//! DISTINCT with no explicit `select` is unaffected (the real
//! projection is already the whole row, so pk-only dedup changes
//! nothing) and must keep working.

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
fn an_explicit_select_with_distinct_under_a_window_refuses_clearly() {
    let run = widget_app().run_ruby(
        "Widget.create!(name: \"a\")\nWidget.create!(name: \"a\")\nb = Widget.create!(name: \"b\")\npage = Widget.select(:name).distinct.order(:id).limit(2)\nbegin\n  before = page.cache_key_with_version\n  b.update_columns(updated_at: Time.now + 3600)\n  after = Widget.select(:name).distinct.order(:id).limit(2).cache_key_with_version\n  puts \"NO_RAISE before=#{before} after=#{after}\"\nrescue ArgumentError => e\n  puts \"ARGUMENT_ERROR: #{e.message}\"\nrescue StandardError => e\n  puts \"OTHER_ERROR: #{e.class}: #{e.message}\"\nend\n",
    );
    run.assert_passes();
    assert!(
        run.stdout.contains("ARGUMENT_ERROR"),
        "an explicit select combined with distinct under a limit/offset window must refuse clearly (ArgumentError), not silently answer a version for a different, pk-deduplicated window: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn distinct_with_no_explicit_select_under_a_window_still_works() {
    let run = widget_app().run_ruby(
        "a = Widget.create!(name: \"a\")\nWidget.create!(name: \"a\")\npage = Widget.distinct.order(:id).limit(2)\nv = page.cache_key_with_version\nputs \"VERSION=#{v}\"\n",
    );
    run.assert_passes();
    assert!(
        run.stdout.contains("VERSION=") && !run.stdout.contains("ARGUMENT_ERROR"),
        "distinct with no explicit select must still answer a version under a limit/offset window (pk-only dedup changes nothing when the real projection is already the whole row): stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
