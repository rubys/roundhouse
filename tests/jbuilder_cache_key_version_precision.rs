//! Regression found by CodeRabbit on roundhouse PR #1034:
//! `ActiveRecord::Base#cache_version` called `to_f` on the RAW stored
//! text `self[:updated_at]` answers for a `datetime`/`timestamp`
//! column (`ColumnSlot::index_read`: only `date` reads its logical
//! value through `[]`; a timestamp's `[]` answers the stored text,
//! `"YYYY-MM-DD HH:MM:SS.ffffff"`). `String#to_f` stops at the first
//! byte that cannot continue a float literal — `-` four digits in does
//! not qualify — so it reads only the leading year: `"2026-10-19
//! 14:17:00".to_f == 2026.0`. Every update within the same YEAR then
//! answered the identical `cache_version`, not merely the same second
//! (the precision fix CodeRabbit's FIRST pass on this PR already
//! covers, confirmed by `tests/jbuilder_cache_key_record_in_key.rs`'s
//! `a_touched_record_gets_a_different_cache_version`, which only moves
//! the clock forward an hour — not far enough to also catch this bug).
//!
//! Two updates eight months apart, both in 2026, must still answer
//! different versions.
//!
//! Exercises `Base#cache_version` DIRECTLY, not through
//! `cache_key_with_version`: every concrete model already gets its own
//! `cache_key_with_version` synthesized straight onto the model class
//! (`src/lower/model_to_library/markers.rs::push_cache_key_methods`,
//! pre-existing, unrelated to this PR), which Ruby's method lookup
//! finds before `ActiveRecord::Base`'s version in `connection.rb` —
//! and that synthesized version embeds the `updated_at` storage ivar's
//! RAW TEXT directly (`"#{cache_key}-#{@updated_at_raw}"`), never
//! calling `cache_version` at all. `Base#cache_version` is still a
//! real, reachable method (anything that calls `.cache_version`
//! directly, or a tableless/abstract model with no per-model
//! override), so the bug is real; it is just invisible through
//! `cache_key_with_version` on an ordinary model, which is why
//! `tests/jbuilder_cache_key_record_in_key.rs`'s own touched-record
//! check (written against `cache_key_with_version`) does not catch it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn updates_within_the_same_year_answer_different_cache_versions() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/account.rb", "class Account < ApplicationRecord\nend\n")
        .run_ruby(
            "account = Account.create!(name: \"acme\")\naccount.update!(updated_at: Time.utc(2026, 1, 1, 0, 0, 0))\nbefore = account.cache_version\naccount.update!(updated_at: Time.utc(2026, 10, 19, 14, 17, 0))\nafter = account.cache_version\nputs \"BEFORE=#{before}\"\nputs \"AFTER=#{after}\"\nputs(before == after ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "two updated_at values in the same year must not collide on the leading year alone: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
