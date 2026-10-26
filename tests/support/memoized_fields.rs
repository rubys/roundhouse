//! Readers that memoize a literal into the ivar of their own name (#1238).

use super::emit_and_run::{self, Overlay};

pub const METER: &str = "class Meter\n  def self.label\n    @label ||= \"x\"\n  end\n\n  def name\n    @name ||= \"n\"\n  end\n\n  def items\n    @items ||= [1, 2]\n  end\nend\n";

const COMMENT_ANCHOR: &str = "  after_destroy_commit { article.broadcast_replace_to(\"articles\") rescue nil }\n";

/// What `[Meter.label, Meter.new.name, Meter.new.items, Comment.new.tone]`
/// answers under CRuby, as JSON.
#[allow(dead_code)]
pub const SUMMARY: &str = r#"["x","n",[1,2],"calm"]"#;

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/meter.rb", METER).edit(
        "app/models/comment.rb",
        COMMENT_ANCHOR,
        &format!("{COMMENT_ANCHOR}\n  def tone\n    @tone ||= \"calm\"\n  end\n"),
    )
}
