//! Not a published gem: the shapes rolify's `rolify`/`resourcify` take, small enough to read in one place.

use super::emit_and_run;

const GEM: &str = "vendor/bundle/ruby/3.4.0/gems/taggy-0.1.0/lib";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("Gemfile.lock", "    debug (1.11.1)\n", "    taggy (0.1.0)\n    debug (1.11.1)\n")
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  taggy\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"taggy\"}\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  taggy\n  taggy_on :topics, [:notes, nil]\n")
        .write(
            &format!("{GEM}/taggy.rb"),
            r##"require "taggy/config"
require "taggy/tagged"
require "taggy/extra"
require "taggy/railtie" if defined?(Rails)

module Taggy
  extend Config

  attr_accessor :tag_cname, :tag_table
  @@kinds = []

  def taggy(options = {})
    include Tagged
    options.reverse_merge!({ cname: "Label" })
    self.tag_cname = options[:cname]
    self.tag_table = tag_cname.tableize.gsub(/\//, "_")
    extra = options.reject { |k, _v| ![:prefix].include?(k.to_sym) }
    @@kinds << self.name if Taggy.registry
  end

  def taggy_on(*kinds)
    include Tagged unless included_modules.include?(Taggy::Tagged)
    kinds.to_a.flatten.compact.map(&:to_sym).each do |kind|
      class_eval <<-RUBY, __FILE__, __LINE__ + 1
        def #{kind}_label
          "#{kind}:" + tag_label
        end
      RUBY
    end
    class_eval do
      def self.taggy_kinds?
        true
      end
    end
  end

  def taggy_from(name)
    class_eval "def #{name.call}_label; end"
  end

  def self.kinds
    @@kinds
  end
end
"##,
        )
        .write(
            &format!("{GEM}/taggy/config.rb"),
            "module Taggy\n  module Config\n    @@registry = true\n\n    def registry\n      @@registry\n    end\n\n    \
             def registry=(value)\n      @@registry = value\n    end\n\n    def disable_registry\n      self.registry = false\n    end\n  end\nend\n",
        )
        .write(&format!("{GEM}/taggy/extra.rb"), "module Taggy\n  def self.extra\n    1\n  end\nend\n")
        .write(
            &format!("{GEM}/taggy/tagged.rb"),
            "module Taggy\n  module Tagged\n    def tag_label\n      \"#{self.class.tag_cname}:#{self.class.tag_table}\"\n    end\n  end\nend\n",
        )
        .write(
            &format!("{GEM}/taggy/railtie.rb"),
            "module Taggy\n  class Railtie < Rails::Railtie\n    initializer \"taggy.active_record\" do\n      \
             ActiveSupport.on_load(:active_record) do\n        ActiveRecord::Base.send :extend, Taggy\n      end\n    end\n  end\nend\n",
        )
}

#[test]
fn a_hook_extended_macro_runs_as_rails_runs_it() {
    app()
        .run_ruby(
            r#"
a = Article.create!(title: "Hello world", body: "body number one is long enough")
raise "tag_label: #{a.tag_label.inspect}" unless a.tag_label == "Label:labels"
raise "kinds: #{Taggy.kinds.inspect}" unless Taggy.kinds == ["Article"]
raise "registry: #{Taggy.registry.inspect}" unless Taggy.registry == true
raise "extra: #{Taggy.extra.inspect}" unless Taggy.extra == 1
got = [a.topics_label, a.notes_label, Article.taggy_kinds?, Article.included_modules.count(Taggy::Tagged)]
raise "taggy_on: #{got.inspect}" unless got == ["topics:Label:labels", "notes:Label:labels", true, 1]
puts "ok"
"#,
        )
        .assert_passes();
}

// Not emitted bare with no diagnostic: the class's superclass is known only once the gem has loaded.
#[test]
fn a_computed_superclass_is_reported() {
    let errors = app()
        .write(&format!("{GEM}/taggy/mark.rb"), "module Taggy\n  class Mark < Taggy.base.constantize\n  end\nend\n")
        .edit(&format!("{GEM}/taggy.rb"), "require \"taggy/extra\"\n", "require \"taggy/extra\"\nrequire \"taggy/mark\"\n")
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("`Taggy::Mark`'s superclass is an expression")), "{errors:#?}");
}

// Not expanded from a guess: the String it `class_eval`s is built when the program runs.
#[test]
fn a_class_eval_of_a_run_time_string_is_reported() {
    let errors = app()
        .edit("app/models/article.rb", "  taggy\n", "  taggy\n  taggy_from -> { \"x\" }\n")
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("`taggy_from` defines methods this call does not expand")), "{errors:#?}");
}
