//! `record.slice(...)` on every target: the typed sites the
//! `record_slice` lowering grounds into a Hash of reader calls. No
//! database: the records are unsaved, so each toolchain only has to
//! compile and run the model. `payload` (the implicit-self site) is
//! compiled, not called: go renders a bare zero-arg call on a local as
//! a field read, so a call would test that heuristic, not `slice`.
//!
//! The last two sites slice a freshly built record, a receiver the
//! lowering binds once, in value position, for two models in one
//! method. `rank` is a nullable integer column, read through a typed
//! receiver into a Hash of mixed values.

use super::emit_and_run;

pub const PROBE: &str = "  validates :title, presence: true

  def payload = { action: \"start\", user: slice(:id, :title, :created_at) }

  def self.sliced
    a = Article.new(id: 7, title: \"Sliced\", body: \"A sufficiently long body.\")
    s = a.slice(:id, :title)
    t = Article.new(id: 8, title: \"Other\", body: \"Another long body.\").slice([:title])
    b = [Article.new(id: 9, title: \"Bound\", body: \"A third long body.\", rank: 5).slice(:id, :title, :rank)]
    c = [Comment.new(id: 4, commenter: \"C\", body: \"A comment body.\").slice(:id, :body)]
    [s.size.to_s, s[\"title\"].to_s, s[\"id\"].to_s, t.size.to_s, t[\"title\"].to_s, a.slice(:id, :title, :created_at).size.to_s,
     b[0][\"title\"].to_s, b[0][\"rank\"].to_s, c[0][\"id\"].to_s, c[0][\"body\"].to_s]
  end
";

/// What CRuby answers for `Article.sliced`, as Rails would.
pub const EXPECTED: [&str; 10] = ["2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."];

pub fn probe_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "    t.string \"title\"\n    t.text \"body\"\n", "    t.string \"title\"\n    t.text \"body\"\n    t.integer \"rank\"\n")
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PROBE)
}
