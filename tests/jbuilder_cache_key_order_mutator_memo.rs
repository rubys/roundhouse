//! Regression found by CodeRabbit on roundhouse PR #1034, companion to
//! `tests/jbuilder_cache_key_relation_memo.rs` (which covers `where!`):
//! `order_parts!`/`clear_ordering` change `@orders` — and therefore
//! `to_sql`, and therefore what `cache_key`'s digest should answer —
//! without always reaching `unload_records` (which clears the
//! `@cache_keys`/`@cache_versions` memos as of this PR's first review
//! round). `order_parts!` only calls it on ONE of its three exit paths
//! (the in-place-resort FAILURE branch); it returns early, memos
//! un-cleared, when the relation's records are not yet loaded (the
//! common case: `rel.cache_key` itself never loads records), and keeps
//! the stale memos on the in-place-resort SUCCESS branch too.
//! `clear_ordering` sets `@records = nil` directly, bypassing
//! `unload_records` entirely.
//!
//! `unscope!` was audited for the same gap and turned out NOT to have
//! it: it already ends with an unconditional `unload_records` call
//! (present before this review round), which already clears both
//! memos via round 1's fix. No change was needed there, and no test is
//! added for it — a test that cannot fail either way is not a
//! regression test.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn order_bang_on_an_unloaded_relation_does_not_reuse_a_stale_cache_key() {
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
            // `rel.cache_key` never loads records (no terminal method
            // called), so `order_parts!`'s `loaded.nil?` early-return
            // path is the one taken here.
            "Widget.create!(name: \"a\")\nrel = Widget.all\nfirst = rel.cache_key\nrel.order!(:name)\nsecond = rel.cache_key\nputs \"FIRST=#{first}\"\nputs \"SECOND=#{second}\"\nputs(first == second ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "order! must not leave a stale cache_key memo when called before the relation is ever loaded: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn clear_ordering_does_not_reuse_a_stale_cache_key() {
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
            "Widget.create!(name: \"a\")\nrel = Widget.order(:name)\nfirst = rel.cache_key\nrel.clear_ordering\nsecond = rel.cache_key\nputs \"FIRST=#{first}\"\nputs \"SECOND=#{second}\"\nputs(first == second ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "clear_ordering must not leave a stale cache_key memo: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
