//! A model's class method calling another without a receiver (#1236).

use super::emit_and_run::{self, Overlay};

const COMMENT_ANCHOR: &str = "  after_destroy_commit { article.broadcast_replace_to(\"articles\") rescue nil }\n";

/// What `[Comment.loud_tag, Comment.tag_size].join(",")` answers under CRuby.
#[allow(dead_code)]
pub const SUMMARY: &str = "T,1";

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().edit(
        "app/models/comment.rb",
        COMMENT_ANCHOR,
        &format!(
            "{COMMENT_ANCHOR}\n  def self.tag\n    \"t\"\n  end\n\n  def self.loud_tag\n    tag().upcase\n  end\n\n  def self.tag_size\n    tag().size\n  end\n"
        ),
    )
}
