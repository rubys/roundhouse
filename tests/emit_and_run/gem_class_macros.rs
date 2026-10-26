//! A source gem's model macro that mixes modules into its caller. The
//! acts_as_follower / audited / ancestry shape: the gem extends
//! ActiveRecord::Base in `ActiveSupport.on_load(:active_record)`, and
//! `acts_as_greeter` in a model body runs `include`/`extend` on that
//! model. The expansion writes those mixins into the model's body.

use super::emit_and_run;

const GEM: &str = "vendor/bundle/ruby/3.4.0/gems/greeter-0.1.0/lib";

fn app(model: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "    debug (1.11.1)\n",
            "    greeter (0.1.0)\n      activesupport (>= 3.0.0)\n    debug (1.11.1)\n",
        )
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  greeter\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"greeter\"}\n")
        .write("config/initializers/greeter.rb", "require \"greeter\"\n")
        .write(
            &format!("{GEM}/greeter.rb"),
            "require \"greeter/model\"\n\nActiveSupport.on_load(:active_record) { extend Greeter::Model }\n",
        )
        .write(&format!("{GEM}/greeter/model.rb"), model)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  acts_as_greeter\n",
        )
}

#[test]
fn a_gem_macro_mixes_its_modules_into_the_model() {
    app(
        "module Greeter
  module Greeting
    def greeting = \"Hello, #{title}\"
  end

  module Finders
    def greeters = all.map(&:greeting)
  end

  module Model
    def acts_as_greeter
      include Greeter::Greeting
      extend Greeter::Finders
    end
  end
end
",
    )
    .run_ruby(
        r#"Article.create!(title: "World", body: "body number one is long enough")
got = [Article.first.greeting, Article.greeters]
raise "got #{got.inspect}" unless got == ["Hello, World", ["Hello, World"]]
puts "ok"
"#,
    )
    .assert_passes();
}

// Not replayed as written: re-read from the model, `include Greeting` would name a top-level constant.
#[test]
fn a_macro_mixin_written_relative_to_its_module_names_the_gems_module() {
    app(
        "module Greeter
  module Greeting
    def greeting = \"Hello, #{title}\"
  end

  module Model
    def acts_as_greeter
      include Greeting
    end
  end
end
",
    )
    .run_ruby(
        r#"Article.create!(title: "World", body: "body number one is long enough")
raise "got #{Article.first.greeting.inspect}" unless Article.first.greeting == "Hello, World"
puts "ok"
"#,
    )
    .assert_passes();
}

#[test]
fn a_class_methods_module_another_module_extends_is_emitted_as_a_module() {
    app(
        "module Greeter
  module Greeting
    def greeting = \"Hello, #{title}\"
  end

  module ClassMethods
    def greeters = all.map(&:greeting)
  end

  module Model
    def acts_as_greeter
      include Greeter::Greeting
      extend Greeter::ClassMethods
    end
  end
end
",
    )
    .run_ruby(
        r#"Article.create!(title: "World", body: "body number one is long enough")
raise "got #{Article.greeters.inspect}" unless Article.greeters == ["Hello, World"]
puts "ok"
"#,
    )
    .assert_passes();
}
