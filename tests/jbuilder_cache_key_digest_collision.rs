//! Regression found by CodeRabbit on roundhouse PR #1034:
//! `Relation#cache_key`'s digest was a bounded multiply-add ROLLING
//! hash (`h = (h * 31 + byte) % modulus`), which collides trivially —
//! "Aa" and "BB" hash identically regardless of what came before them
//! in the string, because `(h*31+65)*31+97` and `(h*31+66)*31+66` both
//! reduce to `961*h + 2112` for ANY prior `h`. Two equal-length SQL
//! queries differing only by a two-character filter VALUE at the same
//! position (`WHERE widgets.name = 'Aa'` vs `'BB'`) therefore got the
//! SAME `cache_key` — and if their versions also matched (plausible:
//! same row count, no `updated_at` column), `cache_key_with_version`
//! would serve one query's cached fragment for the other.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn queries_differing_only_by_the_classic_aa_bb_collision_get_different_keys() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
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
            "aa = Widget.where(name: \"Aa\").cache_key\nbb = Widget.where(name: \"BB\").cache_key\nputs \"AA=#{aa}\"\nputs \"BB=#{bb}\"\nputs(aa == bb ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "two distinct queries (filtering on \"Aa\" vs \"BB\") must not collide on cache_key: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
