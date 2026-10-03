//! Executable Concern accessor contracts and baseline controls.

use super::emit_and_run;

/// A concern's accessors must use each concrete includer's ivars,
/// and must not replace explicit methods defined after the include.
#[test]
fn concern_model_virtual_accessors_run() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/dormant.rb",
            "module Dormant\n  extend ActiveSupport::Concern\n  included { private; attr_accessor :scratch }\nend\n",
        )
        .write(
            "app/models/concerns/draft_state.rb",
            "module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch, :flag, :reader_override, :writer_override\n    nil\n    false\n    42\n    :inert\n    \"inert\"\n  end\n  class_methods do\n    def ordered\n      order(:id)\n    end\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include DraftState\n\n  def scratch_ivar\n    @scratch\n  end\n\n  def set_scratch_ivar(value)\n    @scratch = value\n  end\n\n  private def reader_override\n    \"custom reader\"\n  end\n\n  def reader_override_ivar\n    @reader_override\n  end\n\n  protected def writer_override=(value)\n    @writer_override = \"custom \" + value\n  end\n\n  private \"reader_override\"\n  public :reader_override\n  private :writer_override=\n  public \"writer_override=\"\n",
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n",
            "class Comment < ApplicationRecord\n  private\n  include DraftState\n",
        )
        .write(
            "test/models/concern_accessor_test.rb",
            r#"require "test_helper"
class ConcernAccessorTest < ActiveSupport::TestCase
  test "included accessors use per-record virtual storage" do
    first = Article.new
    second = Article.new
    comment = Comment.new
    loaded = articles(:one)
    assert_nil first.scratch
    assert_nil second.scratch
    assert_nil comment.scratch
    assert_nil loaded.scratch
    first.scratch = "draft"
    assert_equal "draft", first.scratch_ivar
    first.set_scratch_ivar("revised")
    assert_equal "revised", first.scratch
    comment.scratch = "comment"
    loaded.scratch = "loaded"
    assert_nil second.scratch
    assert_equal "revised", first.scratch
    assert_equal "comment", comment.scratch
    assert_equal "loaded", loaded.scratch
    first.flag = false
    assert_equal false, first.flag
    assert_nil comment.flag
    first.scratch = nil
    assert_nil first.scratch
    assert_equal "comment", comment.scratch
    preloaded = Comment.ordered.includes(:article).to_a
    assert_equal Comment.count, preloaded.size
    assert preloaded.size > 0
    preloaded.each do |record|
      assert_equal record.article_id, record.article.id
      record.scratch = "preloaded"
      assert_equal "preloaded", record.scratch
    end
  end

  test "explicit overrides keep their complementary synthesized half" do
    article = Article.new
    article.reader_override = "stored"
    assert_equal "stored", article.reader_override_ivar
    assert_equal "custom reader", article.reader_override
    article.writer_override = "written"
    assert_equal "custom written", article.writer_override
    comment = Comment.new
    comment.reader_override = "plain reader"
    comment.writer_override = "plain writer"
    assert_equal "plain reader", comment.reader_override
    assert_equal "plain writer", comment.writer_override
  end
end
"#,
        )
        .run_test("test/models/concern_accessor_test.rb")
        .assert_passes();
}

/// Intermediate abstract bases still emit the methods their concrete
/// children inherit; admission eligibility must not suppress synthesis.
#[test]
fn abstract_base_accessors_and_typed_attributes_are_inherited() {
    let base = "class ArticleAccessorBase < ApplicationRecord\n  self.abstract_class = true\n  attr_accessor :draft\n  attribute :reviewed, :boolean\nend\n";
    let values = r#"
first = Article.new
second = Article.new
raise "inherited accessor not initially nil" unless first.draft.nil?
first.draft = 'draft'
raise "inherited reader lost value" unless first.draft == 'draft'
raise "records share inherited storage" unless second.draft.nil?
first.reviewed = '0'
second.reviewed = '1'
raise "inherited boolean writer lost false cast" unless first.reviewed == false
raise "inherited boolean writer lost true cast" unless second.reviewed == true
puts 'inherited draft=draft reviewed=false second_reviewed=true'
"#;
    let native = std::process::Command::new("ruby")
        .args(["-e", &format!(r#"
require 'active_record'
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.define {{ create_table(:articles) {{ |t| t.string :title }} }}
class ApplicationRecord < ActiveRecord::Base; self.abstract_class = true; end
{base}
class Article < ArticleAccessorBase; end
{values}
"#)])
        .output()
        .unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert!(String::from_utf8_lossy(&native.stdout).contains("inherited draft=draft reviewed=false second_reviewed=true"));
    let run = emit_and_run::real_blog()
        .write("app/models/article_accessor_base.rb", base)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ArticleAccessorBase\n")
        .run_ruby(values);
    run.assert_passes();
    assert!(run.stdout.contains("inherited draft=draft reviewed=false second_reviewed=true"));
}

/// Direct model declarations already use ordinary per-record ivars.
#[test]
fn direct_model_virtual_accessors_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  attr_accessor :scratch, :flag\n\n  def scratch_ivar\n    @scratch\n  end\n\n  def set_scratch_ivar(value)\n    @scratch = value\n  end\n",
        )
        .write(
            "test/models/virtual_accessor_test.rb",
            r#"require "test_helper"
class VirtualAccessorTest < ActiveSupport::TestCase
  test "virtual attributes use per-instance ivars" do
    fresh = Article.new
    loaded = articles(:one)
    assert_nil fresh.scratch
    assert_nil loaded.scratch
    fresh.scratch = "draft"
    assert_equal "draft", fresh.scratch_ivar
    fresh.set_scratch_ivar("revised")
    assert_equal "revised", fresh.scratch
    fresh.flag = false
    assert_equal false, fresh.flag
    fresh.scratch = nil
    assert_nil fresh.scratch
    loaded.scratch = "loaded"
    assert_nil fresh.scratch
    assert_equal "loaded", loaded.scratch
  end
end
"#,
        )
        .run_test("test/models/virtual_accessor_test.rb")
        .assert_passes();
}

/// An included block is not executed just because its module exists.
#[test]
fn dormant_concern_accessors_do_not_change_runtime() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/dormant.rb",
            "module Dormant\n  extend ActiveSupport::Concern\n  included do\n    private\n    attr_accessor :scratch\n  end\n  def self.included(base)\n    base.define_method(:scratch) { 'from callback' }\n  end\nend\n",
        )
        .run_ruby(r#"
article = Article.new
raise "dormant block executed" if article.respond_to?(:scratch, true)
raise "unrelated body lost" unless article.body.nil?
puts "dormant concern stayed inert"
"#)
        .assert_passes();
}

/// A child accessor replaces an inherited user reader, while parent
/// helpers still share the same per-instance Ruby ivar.
#[test]
fn inherited_user_methods_and_storage_keep_ruby_accessor_semantics() {
    emit_and_run::real_blog()
        .write("app/models/concerns/draft_state.rb", "module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n  end\nend\n")
        .write("app/models/article_accessor_base.rb", "class ArticleAccessorBase < ApplicationRecord\n  self.abstract_class = true\n  def scratch\n    'parent reader'\n  end\n  def set_parent_scratch\n    @scratch = 'from parent'\n  end\n  def parent_storage\n    @scratch\n  end\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ArticleAccessorBase\n  include DraftState\n")
        .run_ruby(r#"
first = Article.new
second = Article.new
raise "parent reader shadowed child accessor" unless first.scratch.nil?
first.set_parent_scratch
raise "parent storage is not shared" unless first.scratch == "from parent"
first.scratch = "child write"
raise "child writer lost parent storage" unless first.parent_storage == "child write"
raise "records share storage" unless second.scratch.nil?
puts "inherited accessor semantics preserved"
"#)
        .assert_passes();
}

/// A fresh virtual attribute must coexist with generated methods and
/// storage on an abstract model ancestor, including loaded records.
#[test]
fn inherited_associations_coexist_with_fresh_concern_accessors() {
    emit_and_run::real_blog()
        .write("app/models/concerns/draft_state.rb", "module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n  end\nend\n")
        .write("app/models/article_accessor_base.rb", "class ArticleAccessorBase < ApplicationRecord\n  self.abstract_class = true\n  has_many :inherited_comments, class_name: 'Comment', foreign_key: :article_id\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ArticleAccessorBase\n  include DraftState\n")
        .write("test/models/inherited_concern_accessor_test.rb", r#"require "test_helper"
class InheritedConcernAccessorTest < ActiveSupport::TestCase
  test "virtual writes leave inherited associations intact" do
    first = articles(:one)
    second = articles(:two)
    assert_nil first.scratch
    assert_nil second.scratch
    assert_equal [comments(:one).id], first.inherited_comments.map(&:id)
    assert_equal [comments(:two).id], second.inherited_comments.map(&:id)
    first.scratch = "first"
    second.scratch = false
    assert_equal "first", first.scratch
    assert_equal false, second.scratch
    assert_equal [comments(:one).id], first.inherited_comments.map(&:id)
    assert_equal [comments(:two).id], second.inherited_comments.map(&:id)
  end
end
"#)
        .run_test("test/models/inherited_concern_accessor_test.rb")
        .assert_passes();
}

/// Inert hooks and the existing consumed class-method carrier must
/// retain the same per-instance values as real ActiveSupport/Record.
#[test]
fn inert_and_consumed_included_hooks_preserve_native_accessor_values() {
    for (hook, carrier) in [
        ("def self.included(base); end", false),
        ("def self.included(base); nil; false; 'ignored'; end", false),
        ("def self.included(base, value = 'ignored'); return 'ignored'; end", false),
        ("def self.included(base); class << base; def label; 'carrier'; end; end; end", true),
        ("module ClassMethods; def label; 'carrier'; end; end", true),
    ] {
        let source = format!("module Virtual\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n  end\n  {hook}\nend\n");
        let carrier_check = if carrier { "raise 'carrier lost' unless Article.label == 'carrier'" } else { "" };
        let values = format!(r#"
first = Article.new
second = Article.new
raise "not initially nil" unless first.scratch.nil?
first.scratch = 'stored'
raise "reader lost stored value" unless first.scratch == 'stored'
raise "writer lost ivar" unless first.instance_variable_get(:@scratch) == 'stored'
raise "records share storage" unless second.scratch.nil?
{carrier_check}
puts "reader=stored ivar=stored second=nil"
"#);
        let native = std::process::Command::new("ruby").args(["-e", &format!(r#"
require 'active_record'
require 'active_support/concern'
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.define {{ create_table(:articles) {{ |t| t.string :title }} }}
class ApplicationRecord < ActiveRecord::Base; self.abstract_class = true; end
{source}
class Article < ApplicationRecord; include Virtual; end
{values}
"#)]).output().unwrap();
        assert!(native.status.success(), "{hook}: {}", String::from_utf8_lossy(&native.stderr));
        assert!(String::from_utf8_lossy(&native.stdout).contains("reader=stored ivar=stored second=nil"));
        let run = emit_and_run::real_blog()
            .write("app/models/concerns/virtual.rb", &source)
            .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Virtual\n")
            .run_ruby(&values);
        run.assert_passes();
        assert!(run.stdout.contains("reader=stored ivar=stored second=nil"));
        if carrier {
            let src = std::fs::read_to_string(run.emitted.join("app/models/article.rb")).unwrap();
            assert!(src.contains("def self.label"), "{src}");
        }
    }
}

/// The frozen candidate overwrote this real callback's reader with a
/// generated ivar getter (native 'from callback', emitted 'stored',
/// zero errors). Strict refusal now stops that false support claim;
/// survey must not re-advertise the discarded writer/getter contract.
#[test]
fn custom_included_override_is_rejected_without_losing_the_callback_in_survey() {
    use roundhouse::ingest::{ingest_app_from_tree, survey};
    const SOURCE: &str = "module Virtual\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n  end\n  def self.included(base)\n    base.define_method(:scratch) { 'from callback' }\n  end\nend\n";

    let native = std::process::Command::new("ruby").args(["-e", &format!(r#"
require 'active_record'
require 'active_support/concern'
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.define {{ create_table(:articles) {{ |t| t.string :title }} }}
class ApplicationRecord < ActiveRecord::Base; self.abstract_class = true; end
{SOURCE}
class Article < ApplicationRecord; include Virtual; end
a = Article.new
a.scratch = 'stored'
raise "callback reader lost" unless a.scratch == 'from callback'
raise "ordinary writer lost" unless a.instance_variable_get(:@scratch) == 'stored'
puts "reader=from callback ivar=stored"
"#)]).output().unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert!(String::from_utf8_lossy(&native.stdout).contains("reader=from callback ivar=stored"));

    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("app/models/concerns/virtual.rb", SOURCE),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  include Virtual\nend\n"),
    ].into_iter().map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec())).collect();
    let err = ingest_app_from_tree(files).unwrap_err();
    assert!(err.to_string().contains("unconsumed included hook"), "{err}");

    survey::activate();
    let run = emit_and_run::real_blog()
        .write("app/models/concerns/virtual.rb", SOURCE)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Virtual\n  def write_scratch\n    @scratch = 'stored'\n  end\n")
        .run_ruby(r#"
a = Article.new
a.write_scratch
raise "callback lost" unless a.scratch == 'from callback'
raise "user storage lost" unless a.instance_variable_get(:@scratch) == 'stored'
raise "refused writer advertised" if a.respond_to?(:scratch=)
puts "survey reader=from callback ivar=stored writer=absent"
"#);
    let gaps = survey::drain();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].to_string().contains("unconsumed included hook"), "{gaps:?}");
    run.assert_passes();
    assert!(run.stdout.contains("survey reader=from callback ivar=stored writer=absent"));
    let src = std::fs::read_to_string(run.emitted.join("app/models/article.rb")).unwrap();
    assert!(!src.contains("def scratch"), "refused getter/writer must not shadow the callback: {src}");
}

/// A primary base made concrete again must synthesize the methods
/// admission promises, without changing intermediate-base inheritance.
#[test]
fn a_concretized_primary_record_emits_its_admitted_accessors() {
    let source = "module Virtual\n  extend ActiveSupport::Concern\n  included { attr_accessor :scratch }\nend\n";
    let values = "first = Article.new\nsecond = Article.new\nraise 'initial accessor not nil' unless first.scratch.nil?\nfirst.scratch = 'draft'\nraise 'lost accessor' unless first.scratch == 'draft'\nraise 'shared storage' unless second.scratch.nil?\nfirst.reviewed = '0'\nsecond.reviewed = '1'\nraise 'lost false cast' unless first.reviewed == false\nraise 'lost true cast' unless second.reviewed == true\nputs 'concrete primary scratch=draft reviewed=false second_reviewed=true'\n";
    let native = std::process::Command::new("ruby")
        .args(["-e", &format!(r#"
require 'active_record'
require 'active_support/concern'
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.define {{ create_table(:articles) {{ |t| t.string :title }} }}
class ApplicationRecord < ActiveRecord::Base; self.abstract_class = true; end
{source}
class Article < ApplicationRecord; primary_abstract_class; self.abstract_class = false; include Virtual; attribute :reviewed, :boolean; end
{values}
"#)])
        .output().unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert!(String::from_utf8_lossy(&native.stdout).contains("concrete primary scratch=draft reviewed=false second_reviewed=true"));
    let run = emit_and_run::real_blog()
        .edit("app/models/application_record.rb", "primary_abstract_class", "self.abstract_class = true")
        .write("app/models/concerns/virtual.rb", source)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  primary_abstract_class\n  self.abstract_class = false\n  include Virtual\n  attribute :reviewed, :boolean\n")
        .run_ruby(values);
    run.assert_passes();
    assert!(run.stdout.contains("concrete primary scratch=draft reviewed=false second_reviewed=true"));
}
