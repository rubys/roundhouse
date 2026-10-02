//! A model concern's class-side methods reach the model when spelled as
//! the vanilla-Ruby `Module#included` hook instead of
//! ActiveSupport::Concern's `class_methods do` / `module ClassMethods`
//! sugar.
//!
//! Procore's shared search concerns (`app/concerns/search_engine/
//! {indexed,procore_search,tool_search,incrementally_backfillable}.rb`,
//! and more) skip `ActiveSupport::Concern` entirely:
//!
//! ```ruby
//! module Indexed
//!   def self.included(klass)
//!     class << klass
//!       def search_index
//!         ...
//!       end
//!     end
//!   end
//! end
//! ```
//!
//! `klass` IS the including class (Ruby calls `included` with it), so
//! `class << klass` reaches the same singleton `class << self` would
//! inside a `class_methods do` block. Before this fix the
//! `SingletonClassNode` opened on a local variable (not `self`) had no
//! `ingest_expr` arm and hit the "unsupported expression node"
//! catch-all deep inside the `included` method's body, failing the
//! WHOLE FILE's ingest — companion test to `tests/concern_class_methods
//! .rs`, which covers the other two spellings.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec())).collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  include Widget::Indexed
end
"#,
        ),
        (
            "app/models/widget/indexed.rb",
            r#"module Widget::Indexed
  def self.included(klass)
    class << klass
      def search_index
        "widget-index"
      end

      def search_writer
        search_index.upcase
      end
    end
  end

  def search_index
    self.class.search_index
  end
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn widget() -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/models/widget.rb"))
        .map(|f| f.content.clone())
        .expect("widget.rb")
}

/// The `class << klass` methods land on the model as class methods,
/// exactly like `module ClassMethods` / `class_methods do` do.
#[test]
fn included_hook_class_methods_reach_the_including_model() {
    let w = widget();
    assert!(
        w.contains("def self.search_index"),
        "`def self.included(klass); class << klass; …` methods land on the model:\n{w}"
    );
    assert!(
        w.contains("def self.search_writer"),
        "every method inside the singleton block lands, not just the first:\n{w}"
    );
}

/// The module's own `included` hook contributes no method of its own —
/// same as `class_methods do` and `module ClassMethods` contribute no
/// `class_methods`/`ClassMethods` method either.
#[test]
fn the_included_hook_itself_does_not_land_as_a_method() {
    let w = widget();
    assert!(
        !w.contains("def self.included"),
        "the `included` hook is absorbed into the class-methods carrier, not emitted itself:\n{w}"
    );
}

/// The instance-side method the concern also declares (unrelated to
/// the singleton block) is untouched — this fix is narrowly scoped to
/// the `included`-hook shape, not a general singleton-class change.
/// Unlike the class side, plain Ruby `include` already provides
/// instance methods at runtime, so the model keeps the real `include`
/// statement rather than a spliced copy — the method is not expected
/// to appear inline in `widget.rb`.
#[test]
fn the_concerns_instance_method_is_still_reachable_via_include() {
    let w = widget();
    assert!(
        w.contains("include Widget::Indexed"),
        "the concern's own `include` is kept so its instance method reaches the model at runtime:\n{w}"
    );
    assert!(
        !w.contains("def search_writer"),
        "search_writer is a CLASS method here (defined inside `class << klass`); it must not \
         also appear as an instance method:\n{w}"
    );
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn visibility_wrapped_hooks_keep_their_class_method_carrier_and_split_bridge() {
    emit_and_run::real_blog()
        .write(
            "app/lib/wrapped_hook.rb",
            r#"module WrappedHook
  private_class_method def self.included(base)
    class << base
      def wrapped_helper; 47; end
    end
  end
end
"#,
        )
        .write(
            "app/lib/split_bridge.rb",
            r#"module SplitBridge
  private_class_method def self.included(base)
    base.extend(ClassMethods)
  end
end
"#,
        )
        .write(
            "app/lib/split_carrier.rb",
            r#"module SplitBridge
  module ClassMethods
    def split_helper; 83; end
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  include WrappedHook, SplitBridge",
        )
        .run_ruby(
            "raise 'wrapped hook lost' unless Article.wrapped_helper == 47\nraise 'split bridge lost' unless Article.split_helper == 83\nputs 'wrapped carriers preserved'",
        )
        .assert_passes();
}
