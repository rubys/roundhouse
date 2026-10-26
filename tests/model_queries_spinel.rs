//! Model queries in the Spinel binary, through `run_spinel_with_db`:
//! the harness DB boot (`Db.configure` / `Db.exec` / assign adapter —
//! same shape as emitted `test/test_helper.rb` and sibling Spinel BOOT
//! pastes) ahead of the script. Without it a query reads an unconfigured
//! adapter and the binary fails with `undefined method 'first' for nil`.
//! Needs a Spinel compiler, so `#[ignore]`d like its sibling suites; CI's
//! `spinel-framework` job runs it with `--ignored` (`scripts/ci-plan.py`'s
//! `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn widgets() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.integer \"score\"\n  end\nend\n",
        )
}

/// `count`, `maximum` and `minimum` on a plain table, against the rows
/// the script creates (Rails answers 3, 7 and 3).
#[test]
#[ignore = "requires the Spinel toolchain"]
fn count_and_extrema_query_the_table() {
    widgets()
        .run_spinel_with_db(
            r#"[3, 7, 5].each { |s| Widget.create!(score: s) }
got = [Widget.count, Widget.maximum(:score), Widget.minimum(:score)]
raise "queries: #{got.inspect}" unless got == [3, 7, 3]
puts "model queries passed"
"#,
        )
        .assert_passes();
}
