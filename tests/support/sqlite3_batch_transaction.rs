//! `SQLite3::Database#execute_batch`, `#transaction` and Float binds,
//! as a schema-owning app (hindmost's conversation index) calls them.
//! Shared by the interpreted lane (CRuby overlay, the sqlite3 gem
//! itself: `tests/emit_and_run/sqlite3_batch_transaction.rs`) and the
//! native one (spinel, `runtime/spinel/sqlite3_database.rb`:
//! `tests/spinel_toolchain/sqlite3_batch_transaction.rs`); both run the
//! same source and print the gem's lines.

use super::emit_and_run::{real_blog, Overlay};

pub const PATH: &str = "app/models/sqlite_batch_probe.rb";

pub const SOURCE: &str = r##"require "fileutils"

class SqliteBatchProbe
  DIR = "tmp/sqlite_batch_probe"

  def self.path
    FileUtils.rm_rf(DIR)
    FileUtils.mkdir_p(DIR)
    File.join(DIR, "batch.sqlite3")
  end

  # `execute_batch`: every statement runs (a `;` inside a string or a
  # trigger body does not end one, a trailing comment is no statement);
  # bind_vars bind to the statement with as many `?`s; the answer is
  # the last statement's first row.
  def self.batch(db)
    schema = db.execute_batch(<<~SQL)
      CREATE TABLE notes (body TEXT, n INTEGER, score REAL);
      CREATE TRIGGER notes_n AFTER INSERT ON notes WHEN NEW.n IS NULL BEGIN
        UPDATE notes SET n = 0 WHERE rowid = NEW.rowid;
      END;
      -- a trailing comment
    SQL
    last = db.execute_batch("INSERT INTO notes VALUES ('a;b', 1, 0.5); SELECT body, n FROM notes;")
    bound = db.execute_batch("INSERT INTO notes VALUES (?, ?, ?); SELECT count(*) FROM notes", [ "c", nil, 2.25 ])
    none = db.execute_batch("UPDATE notes SET n = n")
    [ schema, last, bound, none, db.execute("SELECT body, n FROM notes ORDER BY rowid") ]
  end

  # A Float binds as a REAL.
  def self.floats(db)
    [ db.execute("SELECT body, typeof(score) FROM notes WHERE score = ?", [ 2.25 ]), db.get_first_value("SELECT ? * 2", 1.25) ]
  end

  # `transaction`: the block's value after a commit; a raise rolls back
  # and propagates; without a block the transaction stays open.
  def self.transactions(db)
    committed = db.transaction { |t| t.execute("INSERT INTO notes VALUES (?, ?, ?)", [ "d", 4, 1.0 ]); "committed" }
    raised = begin
      db.transaction do
        db.execute("INSERT INTO notes VALUES ('e', 5, 1.0)")
        raise ArgumentError, "boom"
      end
    rescue ArgumentError => e
      e.message
    end
    after_raise = db.transaction_active?
    opened = db.transaction(:immediate)
    inside = db.transaction_active?
    db.execute("INSERT INTO notes VALUES ('f', 6, 1.0)")
    rolled = db.rollback
    bodies = db.execute("SELECT body FROM notes ORDER BY rowid").map { |row| row[0] }
    [ committed, raised, after_raise, opened, inside, rolled, db.transaction_active?, bodies ]
  end
end
"##;

pub const SCRIPT: &str = r#"db = SQLite3::Database.new(SqliteBatchProbe.path)
puts SqliteBatchProbe.batch(db).inspect
puts SqliteBatchProbe.floats(db).inspect
puts SqliteBatchProbe.transactions(db).inspect
db.close
puts db.closed?
"#;

/// The sqlite3 gem's answers (2.10.1 on CRuby 4.0) for `SCRIPT`.
pub const EXPECTED: &str = concat!(
    "[nil, [\"a;b\", 1], [2], nil, [[\"a;b\", 1], [\"c\", 0]]]\n",
    "[[[\"c\", \"real\"]], 2.5]\n",
    "[\"committed\", \"boom\", false, true, true, true, false, [\"a;b\", \"c\", \"d\"]]\n",
    "true\n",
);

pub fn overlay() -> Overlay {
    real_blog().write(PATH, SOURCE)
}
