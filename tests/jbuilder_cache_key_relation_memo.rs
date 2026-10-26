//! Regressions in `ActiveRecord::Relation#cache_key`/`#cache_version`
//! found by CodeRabbit on roundhouse PR #1034, against the fixes added
//! in `tests/jbuilder_cache_key_with_version.rs`'s companion commits.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `cache_key`/`cache_version` memoize per timestamp column in
/// `@cache_keys`/`@cache_versions`, keyed off `to_sql` at the time of
/// the first call. The bang chain methods (`where!`, `order!`, …, as
/// opposed to the non-bang `where`/`order`/… which now `spawn` a fresh
/// relation — a later roundhouse change than the one this test
/// originally targeted, not this PR's) mutate the SAME relation object
/// in place and call `unload_records` (which drops the loaded-records
/// memo), but until this fix never cleared the cache-key memos too — a
/// second `cache_key_with_version` call on a since-narrowed relation
/// answered the digest of the OLD SQL.
///
/// Compares `rel.cache_key` directly, not the combined
/// `cache_key_with_version` (roundhouse #1034, CodeRabbit): `where!
/// (id: b.id)` ADDS to the existing `id: a.id`, so the narrowed
/// relation matches NO row — its `cache_version` changes from a
/// one-row count to a zero-row count regardless of whether
/// `@cache_keys` still held the stale digest, so comparing the
/// combined key could pass on a row-count change alone while the
/// digest memo itself stayed stale. Two relations over the same
/// table, narrowed to disjoint single rows, must answer DIFFERENT
/// keys; mutating one relation object through both `where!`s must
/// answer those same two different keys, not the first one twice.
#[test]
fn a_mutated_relation_does_not_reuse_a_stale_cache_key() {
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
            "a = Widget.create!(name: \"a\")\nb = Widget.create!(name: \"b\")\nrel = Widget.where(id: a.id)\nfirst = rel.cache_key\nrel.where!(id: b.id)\nsecond = rel.cache_key\nputs first\nputs second\nputs(first == second ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "a relation narrowed after the first cache_key call must not answer the stale digest: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

/// `compute_cache_version` called `maximum(timestamp_column.to_sym)`
/// unconditionally; `maximum` (`extreme`) raises `ArgumentError` for a
/// column the model's schema does not have. A model with no
/// `updated_at` column raised on every `cache_key_with_version` call —
/// including through `cache!`'s eager key evaluation, caching on or
/// off — where this method's own doc comment already promised a nil
/// (count-only) version instead.
#[test]
fn a_relation_over_a_model_without_updated_at_does_not_raise() {
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
            "Widget.create!(name: \"a\")\nputs Widget.all.cache_key_with_version\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("widgets/query-"),
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
