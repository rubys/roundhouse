//! `record[:col] = value` (and `update_attribute`, which goes through it)
//! on a datetime column must write the way the column's own writer does:
//! canonical storage text, and the parse memo of an already-read column
//! dropped. It used to assign the storage ivar directly, so a read column
//! kept serving its old value and a `Time` was stored as its `to_s`.

use super::emit_and_run;

const SCRIPT: &str = r#"
a = Article.create!(title: "Memo", body: "A sufficiently long body.")
t = Time.utc(2020, 1, 2, 3, 4, 5)

# Read first so the parse memo is populated, then overwrite.
raise "starts nil" unless a.reviewed_at.nil?
a[:reviewed_at] = t
raise "stale after Time write: #{a.reviewed_at.inspect}" unless a.reviewed_at == t
raise "storage text: #{a.reviewed_at_raw.inspect}" unless a.reviewed_at_raw == "2020-01-02 03:04:05.000000"

a[:reviewed_at] = nil
raise "stale after nil write: #{a.reviewed_at.inspect}" unless a.reviewed_at.nil?
raise "raw after nil: #{a.reviewed_at_raw.inspect}" unless a.reviewed_at_raw.nil?

# Through the database, on a record that has been read.
a.update_attribute(:reviewed_at, t)
b = Article.find(a.id)
raise "reload lost it: #{b.reviewed_at.inspect}" unless b.reviewed_at == t
raise "stored text: #{b.reviewed_at_raw.inspect}" unless b.reviewed_at_raw.start_with?("2020-01-02 03:04:05")
b.update_attribute(:reviewed_at, nil)
raise "nil write kept the memo: #{b.reviewed_at.inspect}" unless b.reviewed_at.nil?
raise "nil did not persist" unless Article.find(a.id).reviewed_at.nil?
puts "temporal []= passed"
"#;

#[test]
fn index_assign_on_a_datetime_column_normalizes_and_drops_the_memo() {
    let run = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|\n",
            "create_table \"articles\", force: :cascade do |t|\n    t.datetime \"reviewed_at\"\n",
        )
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("temporal []= passed"), "{}", run.stdout);
}
