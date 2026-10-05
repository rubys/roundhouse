//! An unrelated `comments` reader must retain its own `.new` call.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_library_reader_named_like_an_association_keeps_new() {
    emit_and_run::real_blog()
        .write(
            "app/lib/inbox.rb",
            "class Note\n  attr_reader :text\n  def initialize(text)\n    @text = text\n  end\nend\n\nclass Inbox\n  def comments\n    Note\n  end\n\n  def draft(text)\n    comments.new(text)\n  end\n\n  def explicit_draft(text)\n    self.comments.new(text)\n  end\nend\n",
        )
        .run_ruby("raise 'wrong note' unless Inbox.new.draft('hi').text == 'hi'\nraise 'wrong explicit note' unless Inbox.new.explicit_draft('hi').text == 'hi'\nraise 'wrong external note' unless Inbox.new.comments.new('hi').text == 'hi'")
        .assert_passes();
}

#[test]
fn model_class_readers_named_like_an_association_keep_new() {
    emit_and_run::real_blog()
        .write(
            "app/lib/note.rb",
            "class Note\n  attr_reader :text\n  def initialize(text)\n    @text = text\n  end\nend\n",
        )
        .write(
            "app/models/memo2.rb",
            "class Memo2 < ApplicationRecord\n  has_many :comments\n\n  def self.comments\n    Note\n  end\n\n  def class_draft(text)\n    self.class.comments.new(text)\n  end\n\n  def self.draft(text)\n    comments.new(text)\n  end\nend\n",
        )
        .write(
            "app/lib/desk.rb",
            "class Desk\n  def draft(text)\n    Memo2.comments.new(text)\n  end\nend\n",
        )
        .run_ruby("raise 'wrong class note' unless Memo2.new.class_draft('hi').text == 'hi'\nraise 'wrong bare note' unless Memo2.draft('hi').text == 'hi'\nraise 'wrong desk note' unless Desk.new.draft('hi').text == 'hi'")
        .assert_passes();
}
