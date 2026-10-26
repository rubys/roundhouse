//! Regression found by CodeRabbit on roundhouse PR #1034:
//! `Relation#compute_cache_version` computed `count`/`maximum` over the
//! WHOLE relation, ignoring `limit`/`offset`. `count`'s own SQL
//! (`bound_count_sql`) drops LIMIT/OFFSET on purpose ("page totals see
//! the unpaginated result set"), and an aggregate with no GROUP BY
//! collapses to one row regardless of an OUTER LIMIT — neither answers
//! "how many rows, and the latest timestamp, ON THIS PAGE". Real Rails'
//! own `compute_cache_version` wraps a `has_limit_or_offset?` relation
//! in a subquery first for exactly this reason.
//!
//! Setup: widget `a` (page 1, `limit(1)` ordered by id) and widget `b`
//! (off the page) created so `b`'s `updated_at` is the LATEST of the
//! two. Touching `a` (still older than `b`) must change the PAGE's own
//! cache version — the row that actually changed is on the page — even
//! though it does not change the whole table's unwindowed max.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_limited_relations_version_reflects_a_change_on_its_own_page() {
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
            // `update_columns` (no callbacks) rather than `update!`: this
            // runtime's own `fill_timestamps`, like Rails', re-stamps
            // `updated_at` to the actual current time on every `update!`
            // regardless of an explicit `updated_at:` value passed in —
            // so an `update!(updated_at: Time.now + 3600)` would NOT
            // actually land 3600 seconds in the future. Precise control
            // over which row holds the later timestamp needs the
            // no-callbacks write.
            "a = Widget.create!(name: \"a\")\nb = Widget.create!(name: \"b\")\nb.update_columns(updated_at: Time.now + 3600)\npage = Widget.order(:id).limit(1)\nbefore = page.cache_key_with_version\na.update_columns(updated_at: Time.now + 60)\nafter = Widget.order(:id).limit(1).cache_key_with_version\nputs \"BEFORE=#{before}\"\nputs \"AFTER=#{after}\"\nputs(before == after ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "touching the on-page row must change the page's own cache version, even though the off-page row still holds the later timestamp: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
