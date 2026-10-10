//! `ActiveRecord::Locking::Pessimistic#lock!` / `#with_lock` (#644) were
//! unmodeled: the AR catalog had no entries for them, so `roundhouse check`
//! reported `no known method` on both, and the runtime defined neither.
//!
//! SQLite is a single writer with no `SELECT … FOR UPDATE`, so `lock!`
//! reloads (the enclosing transaction already serializes writers) and
//! `with_lock` runs `lock!` then the block inside a transaction, answering
//! the block's value — same contract Rails documents, minus the lock
//! clause's actual SQL effect (the clause itself, `true` or a String, is
//! accepted — see below). Also covers `with_lock`'s Rails 8.1 shape:
//! a trailing options Hash (`isolation:`/`requires_new:`/`joinable:`,
//! Rails' `DatabaseStatements#transaction` keywords) separated from the
//! lock-clause argument, both forwarded to `transaction` rather than
//! raising `ArgumentError`. Nested transactions — `with_lock` or
//! `transaction` called from inside one already open — JOIN the outer
//! one (Rails' default `requires_new: false`): no BEGIN of its own, and
//! an exception from the inner block rolls back the outer transaction's
//! writes too (see `tests/spinel_transaction_connection.rs` and
//! connection.rb's `self.transaction`). Exercised on both the Ruby and
//! Spinel targets; the Spinel case is `#[ignore]`d like its sibling
//! suites (CI's `spinel-framework` job runs it with `--ignored` — see
//! `scripts/ci-plan.py`'s `SPINEL_TESTS`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The issue's own repro shape: a bare `widgets` table, no associations.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

/// `lock!` reloads fresh attributes and returns self; `with_lock` commits
/// the block's writes and answers its value; an exception inside the block
/// rolls the transaction back.
const ASSERTIONS: &str = r#"
widget = Widget.create!(name: "initial")

# An out-of-band write (raw SQL, bypassing the in-memory object) that
# `lock!` must pick up on reload.
Db.exec("UPDATE widgets SET name = 'locked-value' WHERE id = #{widget.id}")
raise "sanity: in-memory copy should still be stale" unless widget.name == "initial"

locked = widget.lock!
raise "lock! should return self" unless locked.equal?(widget)
raise "lock! should reload fresh attributes" unless widget.name == "locked-value"

result = widget.with_lock do
  widget.update!(name: "value-from-block")
  :block_value
end
raise "with_lock should return the block's value" unless result == :block_value
raise "with_lock should commit the block's write" unless Widget.find(widget.id).name == "value-from-block"

raised = false
begin
  widget.with_lock do
    widget.update!(name: "should-not-persist")
    raise "boom"
  end
rescue => e
  raised = true
  raise "wrong exception propagated: #{e.message}" unless e.message == "boom"
end
raise "expected with_lock to re-raise the block's exception" unless raised
raise "with_lock should roll back the block's write on exception" unless Widget.find(widget.id).name == "value-from-block"

# `lock!` also accepts Rails' String locking-clause form
# (`lock!("FOR UPDATE NOWAIT")`); SQLite has no use for it, so it is
# accepted and ignored, same as the default `true`.
clause = Widget.create!(name: "clause-start")
Db.exec("UPDATE widgets SET name = 'clause-value' WHERE id = #{clause.id}")
locked_clause = clause.lock!("FOR UPDATE NOWAIT")
raise "lock! with a String clause should still reload" unless clause.name == "clause-value"
raise "lock! with a String clause should return self" unless locked_clause.equal?(clause)

# `Model.transaction` accepts (and ignores) Rails' three keyword
# options so a real app's call — and `with_lock`'s own forwarding,
# below — doesn't raise `ArgumentError`.
direct_txn_result = Widget.transaction(requires_new: true, isolation: :serializable, joinable: false) do
  :direct_txn_value
end
raise "Model.transaction should accept Rails' transaction options" unless direct_txn_result == :direct_txn_value

opts = Widget.create!(name: "opts-start")
opts_result = opts.with_lock(requires_new: true, isolation: :serializable, joinable: false) do
  opts.update!(name: "opts-value")
  :opts_block_value
end
raise "with_lock should accept Rails' transaction options without raising" unless opts_result == :opts_block_value
raise "with_lock should still commit with options forwarded" unless Widget.find(opts.id).name == "opts-value"

# `with_lock` called inside an already-open `Model.transaction` joins it
# (no nested BEGIN): a raise from the inner block rolls back the outer
# write too, not just the inner one.
joined = Widget.create!(name: "join-start")
raised_join = false
begin
  Widget.transaction do
    joined.update!(name: "outer-write")
    joined.with_lock do
      joined.update!(name: "inner-write")
      raise "boom-join"
    end
  end
rescue => e
  raised_join = true
  raise "wrong exception propagated from joined with_lock: #{e.message}" unless e.message == "boom-join"
end
raise "expected with_lock-in-transaction to re-raise the block's exception" unless raised_join
raise "with_lock joining an open transaction should roll back the outer write too" unless Widget.find(joined.id).name == "join-start"

puts "lock! and with_lock contract passed"
"#;

#[test]
fn lock_and_with_lock_run_on_ruby() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("lock! and with_lock contract passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn lock_and_with_lock_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    let run = app().run_spinel(&script);
    run.assert_passes();
    assert!(run.stdout.contains("lock! and with_lock contract passed"), "{}", run.stdout);
}
